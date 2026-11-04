// Focused evidence for the production prospective-fan guard (guard.h).
//
// One fixture has three parts: a collapse edge whose midpoint placement makes one
// fan triangle's opposite edge stab another fan triangle (a shared-vertex fold), a
// legal placement of the same fan, and a legal shared-edge adjacency. The crossing is
// stated analytically (the remainder of the stab point inside the other triangle), and
// the "before" geometry is checked with a proper separating-axis test that shares no
// code with the guard.

#include "guard.h"

#include <cstdio>
#include <vector>

namespace {

int failures = 0;

void expect(bool condition, const char* name) {
    std::printf("%s %s\n", condition ? "ok  " : "FAIL", name);
    if (!condition) ++failures;
}

Eigen::RowVector3d point(double x, double y, double z) { return {x, y, z}; }

/// A proper (positive-volume) triangle-triangle intersection, used only to state that
/// the fixture's pre-collapse geometry is legal. It does not skip any pair.
bool properly_intersect(const Triangle& first, const Triangle& second, double epsilon = 1e-9) {
    std::vector<Eigen::RowVector3d> axes;
    const auto normal = [](const Triangle& triangle) {
        return (triangle[1] - triangle[0]).cross(triangle[2] - triangle[0]);
    };
    axes.push_back(normal(first));
    axes.push_back(normal(second));
    for (int left = 0; left < 3; ++left) {
        const Eigen::RowVector3d edge = first[(left + 1) % 3] - first[left];
        for (int right = 0; right < 3; ++right) {
            axes.push_back(edge.cross(second[(right + 1) % 3] - second[right]));
        }
    }
    for (const Eigen::RowVector3d& axis : axes) {
        const double length = axis.norm();
        if (length <= 1e-12) continue;
        const Eigen::RowVector3d unit = axis / length;
        double low_first = 1e300, high_first = -1e300, low_second = 1e300, high_second = -1e300;
        for (int corner = 0; corner < 3; ++corner) {
            const double one = unit.dot(first[corner]);
            const double two = unit.dot(second[corner]);
            low_first = std::min(low_first, one);
            high_first = std::max(high_first, one);
            low_second = std::min(low_second, two);
            high_second = std::max(high_second, two);
        }
        if (high_first < low_second - epsilon || high_second < low_first - epsilon) return false;
    }
    return true;
}

/// Convex weights of `probe` inside one triangle (the analytic witness).
bool inside(const Eigen::RowVector3d& probe, const Triangle& triangle, double margin) {
    const Eigen::RowVector3d first = triangle[1] - triangle[0];
    const Eigen::RowVector3d second = triangle[2] - triangle[0];
    const Eigen::RowVector3d delta = probe - triangle[0];
    const double aa = first.squaredNorm();
    const double bb = second.squaredNorm();
    const double ab = first.dot(second);
    const double denominator = aa * bb - ab * ab;
    if (denominator <= 0.0) return false;
    const double u = (bb * first.dot(delta) - ab * second.dot(delta)) / denominator;
    const double v = (aa * second.dot(delta) - ab * first.dot(delta)) / denominator;
    return u > margin && v > margin && (1.0 - u - v) > margin;
}

}  // namespace

int main() {
    // Fixture: one collapse edge (a, b) and two fan triangles that contain a and b.
    //   a = (0,-1.5,0)   b = (0,1.5,0)   midpoint placement M = (0,0,0)
    //   f = (a, A, B) with A = (0.5,0.5,-1), B = (0.5,0.5,1)
    //   g = (b, X, Y) with X = (2,0,0),     Y = (0,2,0)
    // After the collapse f becomes (M,A,B) and g becomes (M,X,Y): they share M, and
    // f's opposite edge A-B stabs g at (0.5,0.5,0), which is strictly inside g.
    Eigen::MatrixXd positions(6, 3);
    positions << 0.0, -1.5, 0.0, 0.0, 1.5, 0.0, 0.5, 0.5, -1.0, 0.5, 0.5, 1.0, 2.0,
        0.0, 0.0, 0.0, 2.0, 0.0;
    Eigen::MatrixXi faces(2, 3);
    faces << 0, 2, 3, 1, 4, 5;
    Eigen::MatrixXi edges(1, 2);
    edges << 0, 1;

    const Eigen::RowVector3d a = positions.row(0);
    const Eigen::RowVector3d b = positions.row(1);
    const Eigen::RowVector3d A = positions.row(2);
    const Eigen::RowVector3d B = positions.row(3);
    const Eigen::RowVector3d X = positions.row(4);
    const Eigen::RowVector3d Y = positions.row(5);
    const Eigen::RowVector3d M = (a + b) * 0.5;
    expect(M.isZero(), "midpoint matches the analytic origin");

    Triangle first_before{a, A, B};
    Triangle second_before{b, X, Y};
    Triangle first_after{M, A, B};
    Triangle second_after{M, X, Y};

    // 1. The pre-collapse pair is legal: no proper intersection.
    expect(!properly_intersect(first_before, second_before), "before geometry does not intersect");

    // 2. The witness is explicit: the stab point lies strictly inside the other triangle.
    const Eigen::RowVector3d witness = point(0.5, 0.5, 0.0);
    expect(inside(witness, second_after, 0.02), "witness point lies inside the target triangle");
    expect(properly_intersect(first_after, second_after), "after geometry intersects at the witness");

    // 3. The production guard refuses exactly this placement.
    Ring folded;
    folded.kept_faces = {0, 1};
    folded.triangles = {first_after, second_after};
    expect(!local_fan_ok(folded, 0, positions, faces, edges), "guard refuses the shared-vertex fold");

    // 4. Legal shared-edge adjacency remains allowed. Faces (a, r, p) and (b, s, r)
    //    share only `r` before the collapse; after it they share the local edge M-r,
    //    with consistent winding and their tips on opposite sides of that edge.
    Eigen::MatrixXd fan(5, 3);
    fan << 0.0, 0.0, 0.0, 0.0, 0.6, 0.0, 1.0, 0.0, 0.0, 0.0, -1.0, 0.0, 0.0, 1.0,
        0.0;
    Eigen::MatrixXi fan_faces(2, 3);
    fan_faces << 0, 2, 3, 1, 4, 2;
    Eigen::MatrixXi fan_edges(1, 2);
    fan_edges << 0, 1;
    Ring adjacent;
    adjacent.kept_faces = {0, 1};
    adjacent.triangles = {Triangle{fan.row(0), fan.row(2), fan.row(3)},
                          Triangle{fan.row(0), fan.row(4), fan.row(2)}};
    expect(local_fan_ok(adjacent, 0, fan, fan_faces, fan_edges),
           "guard allows legal shared-edge adjacency");

    // 5. A genuine fold across that shared edge is refused: the second tip is moved
    //    into the first triangle's interior, so the two triangles overlap.
    Eigen::MatrixXd folded_fan(5, 3);
    folded_fan << 0.0, 0.0, 0.0, 0.0, 0.6, 0.0, 1.0, 0.0, 0.0, 0.0, -1.0, 0.0,
        0.5, -0.4, 0.0;
    Ring overlapping;
    overlapping.kept_faces = {0, 1};
    overlapping.triangles = {Triangle{folded_fan.row(0), folded_fan.row(2), folded_fan.row(3)},
                             Triangle{folded_fan.row(0), folded_fan.row(4), folded_fan.row(2)}};
    expect(!local_fan_ok(overlapping, 0, folded_fan, fan_faces, fan_edges),
           "guard refuses a shared-edge overlap");

    if (failures != 0) {
        std::printf("LOCAL_FAN_GUARD failed=%d\n", failures);
        return 1;
    }
    std::printf(
        "LOCAL_FAN_GUARD ok witness=(0.5,0.5,0) before_pairs_intersect=0 fold_refused=1 "
        "legal_adjacency_allowed=1 shared_edge_overlap_refused=1\n");
    return 0;
}
