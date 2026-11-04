// Protected native reduction: the libigl geometric-quadric collapse with
// prospective protection, as one self-contained operation.
//
// The shared Rust core owns source selection, opening identity, profile limits and
// final qualification. This executable owns only the reduction operation: it consumes
// a triangular display-frame OBJ, a target face count, a positive source-space
// deviation tolerance and a finite time bound, and it writes a compact candidate plus
// a JSON receipt. Every guard is a *prospective* check on the one-ring a collapse would
// create, so a rejected attempt leaves the mesh untouched.
//
// The libigl headers are supplied by the caller (no fetch). The build embeds a content
// digest of the tree it read; the upstream revision b41f1944a703653ae0da81e6588650ba14c270ea
// is recorded as the revision that tree was qualified against, not as a measured fact.
// Eigen and nlohmann/json are supplied the same way. This file contains no host path,
// garment name or vertex number.
//
// The collapse cost is `igl::qslim` (geometric quadric error): one operation, no
// dual-cost switch and no runtime fallback. Which cost is qualified is profile-level
// engineering history, kept in the issue journal rather than in the tool.
//
// Exit codes: 0 target reached, 2 target not reached, 1 invalid input/IO, 124 deadline.

#include <Eigen/Core>
#include <igl/AABB.h>
#include <igl/block_self_intersections.h>
#include <igl/circulation.h>
#include <igl/default_num_threads.h>
#include <igl/edge_flaps.h>
#include <igl/point_simplex_squared_distance.h>
#include <igl/qslim.h>
#include "guard.h"
#include <nlohmann/json.hpp>

#include <algorithm>
#include <array>
#include <chrono>
#include <cmath>
#include <csignal>
#include <cstdint>
#include <filesystem>
#include <fstream>
#include <iomanip>
#include <iostream>
#include <limits>
#include <map>
#include <set>
#include <sstream>
#include <stdexcept>
#include <string>
#include <vector>

namespace fs = std::filesystem;
using Json = nlohmann::json;

namespace {

volatile std::sig_atomic_t stop_requested = 0;

void request_stop(int) { stop_requested = 1; }

/// A deadline reached inside the callback: the caller checkpoints and reports it.
struct Deadline {};

/// One source mesh in the exported display frame (Y-up), as the OBJ carries it.
struct Mesh {
    Eigen::MatrixXd positions;
    Eigen::MatrixXi faces;
};

/// Read a triangle-only OBJ; anything the operation cannot represent is refused.
Mesh read_obj(const std::string& path) {
    std::ifstream in(path);
    if (!in) throw std::runtime_error("cannot read " + path);
    std::vector<std::array<double, 3>> points;
    std::vector<std::array<int, 3>> faces;
    std::string line;
    while (std::getline(in, line)) {
        std::istringstream row(line);
        std::string tag;
        row >> tag;
        if (tag != "v" && tag != "f") continue;
        if (tag == "v") {
            std::array<double, 3> point{};
            if (!(row >> point[0] >> point[1] >> point[2]))
                throw std::runtime_error("bad vertex record in " + path);
            points.push_back(point);
        } else {
            // Supported form only: three corners, each a bare positive vertex index.
            std::vector<std::string> corners;
            std::string corner;
            while (row >> corner) corners.push_back(corner);
            if (corners.size() != 3)
                throw std::runtime_error("face has " + std::to_string(corners.size()) +
                                         " corners in " + path + "; only triangles are supported");
            std::array<int, 3> face{};
            for (int slot = 0; slot < 3; ++slot) {
                const std::string& text = corners[static_cast<size_t>(slot)];
                const bool bare = !text.empty() &&
                                  std::all_of(text.begin(), text.end(), [](unsigned char letter) {
                                      return letter >= '0' && letter <= '9';
                                  });
                if (!bare)
                    throw std::runtime_error("face corner '" + text +
                                             "' is not a bare positive vertex index in " + path);
                try {
                    face[static_cast<size_t>(slot)] = std::stoi(text) - 1;
                } catch (const std::exception&) {
                    throw std::runtime_error("face index out of range in " + path);
                }
            }
            faces.push_back(face);
        }
    }
    if (points.empty()) throw std::runtime_error("no vertices in " + path);
    if (faces.empty()) throw std::runtime_error("no triangles in " + path);
    Mesh mesh;
    mesh.positions.resize(static_cast<int>(points.size()), 3);
    mesh.faces.resize(static_cast<int>(faces.size()), 3);
    for (size_t i = 0; i < points.size(); ++i)
        for (int axis = 0; axis < 3; ++axis) mesh.positions(static_cast<int>(i), axis) = points[i][axis];
    for (size_t i = 0; i < faces.size(); ++i) {
        for (int corner = 0; corner < 3; ++corner) {
            const int vertex = faces[i][corner];
            if (vertex < 0 || vertex >= static_cast<int>(points.size()))
                throw std::runtime_error("face index out of range in " + path);
            mesh.faces(static_cast<int>(i), corner) = vertex;
        }
        const Eigen::RowVector3i& face = mesh.faces.row(static_cast<int>(i));
        if (face[0] == face[1] || face[1] == face[2] || face[2] == face[0])
            throw std::runtime_error("triangle repeats a corner in " + path);
    }
    if (!mesh.positions.allFinite()) throw std::runtime_error("nonfinite vertex in " + path);
    return mesh;
}

/// Write the live faces of one state, compacting unreferenced vertices first.
///
/// `real_faces` bounds the original rows: rows beyond it, and rows libigl marks with
/// three zero corners, are not surface and are never written. Returns the written
/// counts so the receipt cannot disagree with the file.
std::pair<int, int> write_compact(const std::string& path, const Eigen::MatrixXd& positions,
                                  const Eigen::MatrixXi& faces, int real_faces) {
    std::vector<int> kept;
    kept.reserve(static_cast<size_t>(real_faces));
    std::vector<char> referenced(static_cast<size_t>(positions.rows()), 0);
    for (int face = 0; face < real_faces; ++face) {
        if ((faces.row(face).array() == 0).all()) continue;
        kept.push_back(face);
        for (int corner = 0; corner < 3; ++corner) referenced[static_cast<size_t>(faces(face, corner))] = 1;
    }
    if (kept.empty()) throw std::runtime_error("no live faces to write");
    std::vector<int> remap(static_cast<size_t>(positions.rows()), -1);
    int vertices = 0;
    for (int vertex = 0; vertex < positions.rows(); ++vertex)
        if (referenced[static_cast<size_t>(vertex)]) remap[static_cast<size_t>(vertex)] = vertices++;
    if (vertices == 0) throw std::runtime_error("no referenced vertices to write");
    const std::string temporary = path + ".temporary";
    {
        std::ofstream out(temporary);
        if (!out) throw std::runtime_error("cannot write " + temporary);
        out << std::setprecision(17);
        for (int vertex = 0; vertex < positions.rows(); ++vertex) {
            if (!referenced[static_cast<size_t>(vertex)]) continue;
            if (!positions.row(vertex).allFinite()) throw std::runtime_error("nonfinite output vertex");
            out << "v " << positions(vertex, 0) << " " << positions(vertex, 1) << " "
                << positions(vertex, 2) << "\n";
        }
        for (int face : kept) {
            out << "f " << remap[static_cast<size_t>(faces(face, 0))] + 1 << " "
                << remap[static_cast<size_t>(faces(face, 1))] + 1 << " "
                << remap[static_cast<size_t>(faces(face, 2))] + 1 << "\n";
        }
    }
    fs::rename(temporary, path);
    return {vertices, static_cast<int>(kept.size())};
}

struct Guards {
    size_t attempts = 0;
    size_t collapses = 0;
    size_t blocked_upstream = 0;
    size_t blocked_nonfinite = 0;
    size_t blocked_fan = 0;
    size_t blocked_source = 0;
};

/// One reduction run: the original tree, the deadline, the checkpoint and the guards.
class Run {
  public:
    Run(const Mesh& input, std::string directory, double tolerance, double budget)
        : input_(input),
          directory_(std::move(directory)),
          tolerance_(tolerance),
          budget_(budget) {
        reference_.init(input_.positions, input_.faces);
        started_ = std::chrono::steady_clock::now();
    }

    double seconds() const {
        return std::chrono::duration<double>(std::chrono::steady_clock::now() - started_).count();
    }

    Guards& guards() { return guards_; }

    /// Write a candidate + progress whenever the checkpoint interval has passed, and
    /// save the latest state before a deadline is reported.
    void checkpoint(const Eigen::MatrixXd& positions, const Eigen::MatrixXi& faces, bool force) {
        const double now = seconds();
        const bool stopping = stop_requested || now >= budget_;
        if (force || stopping || now - last_saved_ >= 30.0) {
            write_compact(directory_ + "/candidate.obj", positions, faces, real_faces_);
            std::ofstream(directory_ + "/progress.json")
                << Json{{"seconds", now},
                        {"attempts", guards_.attempts},
                        {"collapses", guards_.collapses},
                        {"blocked_upstream", guards_.blocked_upstream},
                        {"blocked_nonfinite", guards_.blocked_nonfinite},
                        {"blocked_fan", guards_.blocked_fan},
                        {"blocked_source", guards_.blocked_source}}
                       .dump(2)
                << '\n';
            last_saved_ = now;
        }
        if (stopping) throw Deadline{};
    }

    /// Refuse a collapse whose new one-ring leaves the source surface or crosses it.
    bool protects(const Ring& ring, int edge, const Eigen::MatrixXd& positions,
                  const Eigen::MatrixXi& faces, const Eigen::MatrixXi& edges) {
        if (!local_fan_ok(ring, edge, positions, faces, edges)) {
            ++guards_.blocked_fan;
            return false;
        }
        for (const Triangle& triangle : ring.triangles) {
            // Corner, edge-midpoint and centroid samples: a collapse may not move the
            // surface further than the declared tolerance from the original input.
            const std::array<Eigen::RowVector3d, 7> samples = {
                triangle[0],
                triangle[1],
                triangle[2],
                (triangle[0] + triangle[1]) * 0.5,
                (triangle[1] + triangle[2]) * 0.5,
                (triangle[2] + triangle[0]) * 0.5,
                (triangle[0] + triangle[1] + triangle[2]) / 3.0};
            for (const Eigen::RowVector3d& sample : samples) {
                int face = -1;
                Eigen::RowVector3d nearest;
                const double distance = reference_.squared_distance(input_.positions, input_.faces,
                                                                    sample, face, nearest);
                if (!std::isfinite(distance) || distance > tolerance_ * tolerance_) {
                    ++guards_.blocked_source;
                    return false;
                }
            }
        }
        return true;
    }

    void set_real_faces(int real_faces) { real_faces_ = real_faces; }

  private:
    const Mesh& input_;
    std::string directory_;
    double tolerance_;
    double budget_;
    igl::AABB<Eigen::MatrixXd, 3> reference_;
    std::chrono::steady_clock::time_point started_;
    double last_saved_ = -60.0;
    int real_faces_ = 0;
    Guards guards_;
};

bool parse_double(const std::string& text, double& value) {
    try {
        value = std::stod(text, nullptr);
    } catch (const std::exception&) {
        return false;
    }
    return std::isfinite(value);
}

bool parse_int(const std::string& text, int& value) {
    try {
        value = std::stoi(text, nullptr);
    } catch (const std::exception&) {
        return false;
    }
    return true;
}

}  // namespace

int main(int argc, char** argv) {
    try {
        std::map<std::string, std::string> options;
        for (int index = 1; index < argc; ++index) {
            const std::string key = argv[index];
            if (key.rfind("--", 0) != 0) throw std::runtime_error("unexpected argument " + key);
            if (index + 1 >= argc) throw std::runtime_error("missing value for " + key);
            options[key] = argv[++index];
        }
        for (const std::string& required : {"--input", "--target", "--tolerance", "--timeout", "--output"})
            if (!options.count(required)) throw std::runtime_error("missing " + required);
        for (const auto& [key, value] : options)
            if (key != "--input" && key != "--target" && key != "--tolerance" && key != "--timeout" &&
                key != "--output" && key != "--threads")
                throw std::runtime_error("unknown option " + key);
        int target = 0;
        if (!parse_int(options["--target"], target) || target < 1)
            throw std::runtime_error("--target must be a positive triangle count");
        double tolerance = 0.0;
        if (!parse_double(options["--tolerance"], tolerance) || tolerance <= 0.0)
            throw std::runtime_error("--tolerance must be a positive finite length");
        double budget = 0.0;
        if (!parse_double(options["--timeout"], budget) || budget <= 0.0)
            throw std::runtime_error("--timeout must be a positive finite number of seconds");
        int threads = 1;
        if (options.count("--threads") && (!parse_int(options["--threads"], threads) || threads < 1))
            throw std::runtime_error("--threads must be a positive integer");
        const std::string directory = options["--output"];
        if (!fs::create_directory(directory)) throw std::runtime_error("output directory must be fresh");
        igl::default_num_threads(threads);
        std::signal(SIGTERM, request_stop);
        std::signal(SIGINT, request_stop);

        Mesh input = read_obj(options["--input"]);
        if (target >= input.faces.rows())
            throw std::runtime_error("--target must be below the input triangle count; nothing to reduce");
        auto run = std::make_shared<Run>(input, directory, tolerance, budget);
        std::vector<igl::decimate_pre_post_collapse_callbacks_decorator> decorators;
        decorators.push_back(igl::block_self_intersections());
        // The decorator's callback parameters follow the pinned libigl signature; they are
        // taken generically so the guard code never drifts from the qualified header.
        decorators.push_back([run](const auto& positions, const auto& faces, int real_faces,
                                   auto& pre, auto& post) {
            run->set_real_faces(real_faces);
            run->checkpoint(positions, faces, true);
            auto upstream_pre = pre;
            auto upstream_post = post;
            pre = [run, upstream_pre, real_faces](const auto& positions, const auto& faces,
                                                 const auto& edges, const auto& edge_map,
                                                 const auto& edge_faces, const auto& edge_indices,
                                                 const auto& placements, const auto& queue_positions,
                                                 const auto& costs, int edge) {
                run->checkpoint(positions, faces, false);
                ++run->guards().attempts;
                // `costs` carries the placement libigl would use for this edge.
                const Eigen::RowVector3d placement = costs.row(edge);
                if (!placement.allFinite()) {
                    ++run->guards().blocked_nonfinite;
                    return false;
                }
                // The upstream decorator (global self-intersection blocking) and the
                // topological validity of the collapse both answer here.
                if (!upstream_pre(positions, faces, edges, edge_map, edge_faces, edge_indices, placements,
                                  queue_positions, costs, edge)) {
                    ++run->guards().blocked_upstream;
                    return false;
                }
                const Ring ring = prospective(edge, placement, positions, faces, edges, edge_map,
                                              edge_faces, edge_indices, real_faces);
                return run->protects(ring, edge, positions, faces, edges);
            };
            post = [run, upstream_post](const auto& positions, const auto& faces, const auto& edges,
                                        const auto& edge_map, const auto& edge_faces,
                                        const auto& edge_indices, const auto& placements,
                                        const auto& queue_positions, const auto& costs, int edge,
                                        int first, int second, int face_first, int face_second,
                                        bool collapsed) {
                upstream_post(positions, faces, edges, edge_map, edge_faces, edge_indices, placements,
                              queue_positions, costs, edge, first, second, face_first, face_second, collapsed);
                if (collapsed) ++run->guards().collapses;
                run->checkpoint(positions, faces, false);
            };
        });

        Eigen::MatrixXd output_positions;
        Eigen::MatrixXi output_faces;
        Eigen::VectorXi births;
        Eigen::VectorXi origins;
        bool reached = false;
        bool timed_out = false;
        std::string stop_reason = "target";
        try {
            reached = igl::qslim(input.positions, input.faces, target, decorators, output_positions,
                                 output_faces, births, origins);
            if (output_positions.rows() == 0 || output_faces.rows() == 0)
                throw std::runtime_error("reduction returned no mesh; the last checkpoint is kept");
            if (!output_positions.allFinite()) throw std::runtime_error("reduction returned nonfinite data");
            write_compact(directory + "/candidate.obj", output_positions, output_faces,
                          output_faces.rows());
            if (!reached) stop_reason = "budget_exhausted";
        } catch (const Deadline&) {
            timed_out = true;
            stop_reason = stop_requested ? "signal" : "deadline";
        }
        const Json receipt = {
            {"backend", "libigl-protected"},
            {"operation", "igl::qslim geometric quadric collapse"},
            {"cost", "qslim"},
            {"guards_note", "prospective local-fan and source-distance checks around the upstream global self-intersection blocker"},
            {"libigl",
             {{"content_sha256", PROTECTED_REDUCE_IGL_TREE_SHA256},
              {"file_count", PROTECTED_REDUCE_IGL_TREE_FILES},
              {"content_scope", "every .h/.hpp/.cpp file in the libigl include tree this build read"},
              {"content_method", "SHA256 over the sorted 'relative-path sha256' lines of those files"},
              {"qualified_reference", "b41f1944a703653ae0da81e6588650ba14c270ea"},
              {"qualified_reference_note", "upstream revision the tree was qualified against; not measured by this build"}}},
            {"target_triangles", target},
            {"target_reached", reached},
            {"stop_reason", stop_reason},
            {"timed_out", timed_out},
            {"timeout_seconds", budget},
            {"threads", threads},
            {"seconds", run->seconds()},
            {"guards", {{"attempts", run->guards().attempts},
                        {"collapses", run->guards().collapses},
                        {"blocked_upstream", run->guards().blocked_upstream},
                        {"blocked_nonfinite", run->guards().blocked_nonfinite},
                        {"blocked_fan", run->guards().blocked_fan},
                        {"blocked_source", run->guards().blocked_source}}},
            {"source_tolerance", tolerance},
            {"source_vertices", input.positions.rows()},
            {"source_faces", input.faces.rows()},
            {"candidate_vertices", 0},
            {"candidate_faces", 0}};
        Json written = receipt;
        {
            Mesh candidate = read_obj(directory + "/candidate.obj");
            written["candidate_vertices"] = candidate.positions.rows();
            written["candidate_faces"] = candidate.faces.rows();
        }
        std::ofstream(directory + "/result.json") << written.dump(2) << '\n';
        std::cout << written.dump() << std::endl;
        if (timed_out) return 124;
        return reached ? 0 : 2;
    } catch (const std::exception& error) {
        std::cerr << error.what() << std::endl;
        return 1;
    }
}
