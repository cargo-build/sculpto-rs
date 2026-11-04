// Protection guards shared by the protected reducer and its focused guard test.
//
// These are the production checks: the local one-ring orientation sign and the
// shared-edge/shared-vertex crossing predicates for the one-ring a collapse would
// create, plus the predicted one-ring itself. The tool compiles this header; so does
// the guard test, so the tested code is the code that runs.
//
// libigl's global blocker excludes the changed one-ring from its own tree, which is why
// the local checks exist at all.

#pragma once

#include <Eigen/Core>
#include <igl/circulation.h>
#include <igl/point_simplex_squared_distance.h>
#include <igl/triangle_triangle_intersect_shared_edge.h>
#include <igl/triangle_triangle_intersect_shared_vertex.h>

#include <algorithm>
#include <array>
#include <cmath>
#include <map>
#include <stdexcept>
#include <utility>
#include <vector>

using Triangle = std::array<Eigen::RowVector3d, 3>;

inline double point_triangle_sq(const Eigen::RowVector3d& point, const Triangle& triangle) {
    Eigen::Matrix<double, 3, 3> vertices;
    for (int corner = 0; corner < 3; ++corner) vertices.row(corner) = triangle[corner];
    Eigen::Matrix<int, 1, 3> simplex;
    simplex << 0, 1, 2;
    Eigen::RowVector3d nearest;
    double distance = 0.0;
    igl::point_simplex_squared_distance<3>(point, vertices, simplex, 0, distance, nearest);
    return distance;
}

/// The faces one collapse would replace, with the new corners already placed.
struct Ring {
    std::vector<int> old_faces;
    std::vector<int> kept_faces;
    std::vector<Triangle> triangles;
};

/// Predict the one-ring of collapsing `edge` to `placement`, from libigl's live maps.
inline Ring prospective(int edge, const Eigen::RowVector3d& placement, const Eigen::MatrixXd& positions,
                 const Eigen::MatrixXi& faces, const Eigen::MatrixXi& edges,
                 const Eigen::VectorXi& edge_map, const Eigen::MatrixXi& edge_faces,
                 const Eigen::MatrixXi& edge_indices, int real_faces) {
    Ring ring;
    std::vector<int> circulation_vertices, circulation_faces;
    igl::circulation(edge, true, faces, edge_map, edge_faces, edge_indices, circulation_vertices,
                     circulation_faces);
    ring.old_faces = circulation_faces;
    igl::circulation(edge, false, faces, edge_map, edge_faces, edge_indices, circulation_vertices,
                     circulation_faces);
    ring.old_faces.insert(ring.old_faces.end(), circulation_faces.begin(), circulation_faces.end());
    std::sort(ring.old_faces.begin(), ring.old_faces.end());
    ring.old_faces.erase(std::unique(ring.old_faces.begin(), ring.old_faces.end()), ring.old_faces.end());
    for (int face : ring.old_faces) {
        // The two collapsing faces disappear; only faces inside the real range can be
        // re-used, so libigl's fictitious boundary faces never become surface.
        if (face >= real_faces || face == edge_faces(edge, 0) || face == edge_faces(edge, 1)) continue;
        Triangle triangle;
        for (int corner = 0; corner < 3; ++corner) {
            const int vertex = faces(face, corner);
            triangle[corner] = (vertex == edges(edge, 0) || vertex == edges(edge, 1))
                                   ? placement
                                   : positions.row(vertex);
        }
        ring.kept_faces.push_back(face);
        ring.triangles.push_back(triangle);
    }
    return ring;
}

/// Keep the prospective one-ring free of flipped and self-crossing faces.
///
/// libigl's global callback excludes the changed one-ring from its own tree, so the
/// orientation sign and the shared-edge/shared-vertex crossing predicates are checked
/// here instead. A fan that loses its common vertex or crosses itself is refused.
inline bool local_fan_ok(const Ring& ring, int edge, const Eigen::MatrixXd& positions,
                  const Eigen::MatrixXi& faces, const Eigen::MatrixXi& edges) {
    std::map<int, int> ids;
    std::vector<Eigen::RowVector3d> points;
    Eigen::MatrixXi fan(static_cast<int>(ring.kept_faces.size()), 3);
    const int merged = std::min(edges(edge, 0), edges(edge, 1));
    for (size_t index = 0; index < ring.kept_faces.size(); ++index) {
        const int face = ring.kept_faces[index];
        const Eigen::Vector3d old_a = (positions.row(faces(face, 1)) - positions.row(faces(face, 0))).transpose();
        const Eigen::Vector3d old_b = (positions.row(faces(face, 2)) - positions.row(faces(face, 0))).transpose();
        const Eigen::Vector3d new_a = (ring.triangles[index][1] - ring.triangles[index][0]).transpose();
        const Eigen::Vector3d new_b = (ring.triangles[index][2] - ring.triangles[index][0]).transpose();
        if (old_a.cross(old_b).dot(new_a.cross(new_b)) <= 0.0) return false;
        for (int corner = 0; corner < 3; ++corner) {
            int key = faces(face, corner);
            if (key == edges(edge, 0) || key == edges(edge, 1)) key = merged;
            const auto [slot, inserted] = ids.emplace(key, static_cast<int>(ids.size()));
            if (inserted) points.push_back(ring.triangles[index][corner]);
            fan(static_cast<int>(index), corner) = slot->second;
        }
    }
    Eigen::MatrixXd placed(static_cast<int>(points.size()), 3);
    for (size_t index = 0; index < points.size(); ++index) placed.row(static_cast<int>(index)) = points[index];
    for (int first = 0; first < fan.rows(); ++first) {
        for (int second = first + 1; second < fan.rows(); ++second) {
            std::vector<std::pair<int, int>> shared;
            for (int left = 0; left < 3; ++left)
                for (int right = 0; right < 3; ++right)
                    if (fan(first, left) == fan(second, right)) shared.emplace_back(left, right);
            if (shared.size() == 3) return false;
            if (shared.size() == 2) {
                int corner = 0;
                while (corner == shared[0].first || corner == shared[1].first) ++corner;
                const Eigen::RowVector3d tip = placed.row(fan(first, corner));
                if (igl::triangle_triangle_intersect_shared_edge(placed, fan, first, corner, tip,
                                                                 second, 1e-8))
                    return false;
            } else if (shared.size() == 1) {
                const int shared_first = shared[0].first;
                const int shared_second = shared[0].second;
                const int corner = (shared_first + 1) % 3;
                const Eigen::RowVector3d tip = placed.row(fan(first, corner));
                if (igl::triangle_triangle_intersect_shared_vertex(placed, fan, first, shared_first,
                                                                   corner, tip, second, shared_second,
                                                                   1e-14))
                    return false;
            } else {
                throw std::runtime_error("prospective one-ring lost its common vertex");
            }
        }
    }
    return true;
}

/// Counters the receipt reports; every guard names itself, so a silent pass is visible.
