# Protected native reduction (optional constructor backend).
#
# The operation is libigl's geometric-quadric collapse (`igl::qslim`) with prospective
# one-ring protection; it is not a new optimizer. Headers only, no fetch:
#
#   libigl  qualified against b41f1944a703653ae0da81e6588650ba14c270ea
#           (the build records a content digest of the tree it actually reads)
#   Eigen   any recent 3.3+ release                    (header only)
#   nlohmann/json  any 3.9+ single-header release
#
# Configure with the pinned include directories, for example:
#
#   cmake -S tools/native -B build/native \
#     -DIGL_INCLUDE_DIR=<pin>/include \
#     -DEIGEN3_INCLUDE_DIR=<eigen> \
#     -DNLOHMANN_JSON_INCLUDE_DIR=<nlohmann_json>/include \
#     -DCMAKE_BUILD_TYPE=Release
#   cmake --build build/native --target protected_reduce protected_reduce_guard_test -j 2
#
# No path in this file is host specific; the caller supplies them.

cmake_minimum_required(VERSION 3.20)
project(protected_reduce LANGUAGES CXX)

set(CMAKE_CXX_STANDARD 17)
set(CMAKE_CXX_STANDARD_REQUIRED ON)
set(CMAKE_CXX_EXTENSIONS OFF)

find_path(
  IGL_INCLUDE_DIR
  NAMES igl/decimate.h
  HINTS ${IGL_INCLUDE_DIR}
  DOC "libigl include directory (qualified pin b41f1944a703653ae0da81e6588650ba14c270ea)"
)
find_path(
  EIGEN3_INCLUDE_DIR
  NAMES Eigen/Core
  HINTS ${EIGEN3_INCLUDE_DIR}
  DOC "Eigen include directory"
)
find_path(
  NLOHMANN_JSON_INCLUDE_DIR
  NAMES nlohmann/json.hpp
  HINTS ${NLOHMANN_JSON_INCLUDE_DIR}
  DOC "nlohmann/json include directory"
)

if(NOT IGL_INCLUDE_DIR)
  message(FATAL_ERROR "IGL_INCLUDE_DIR must point at the qualified libigl include directory")
endif()
if(NOT EIGEN3_INCLUDE_DIR)
  message(FATAL_ERROR "EIGEN3_INCLUDE_DIR must point at an Eigen include directory")
endif()
if(NOT NLOHMANN_JSON_INCLUDE_DIR)
  message(FATAL_ERROR "NLOHMANN_JSON_INCLUDE_DIR must point at a nlohmann/json include directory")
endif()

find_package(Threads REQUIRED)

# The build embeds the content identity of the tree it reads, not a revision name.
include(${CMAKE_CURRENT_SOURCE_DIR}/igl_identity.cmake)
message(STATUS "libigl tree: ${IGL_TREE_FILE_COUNT} files, content ${IGL_TREE_SHA256}")

add_executable(protected_reduce protected_reduce.cpp)
target_include_directories(
  protected_reduce
  SYSTEM PRIVATE ${IGL_INCLUDE_DIR} ${EIGEN3_INCLUDE_DIR} ${NLOHMANN_JSON_INCLUDE_DIR}
)
target_compile_options(protected_reduce PRIVATE -O2 -DNDEBUG)
target_compile_definitions(
  protected_reduce
  PRIVATE PROTECTED_REDUCE_IGL_TREE_SHA256="${IGL_TREE_SHA256}"
          PROTECTED_REDUCE_IGL_TREE_FILES=${IGL_TREE_FILE_COUNT}
)
target_link_libraries(protected_reduce PRIVATE Threads::Threads)

# Focused evidence for the production guard; not installed and not user facing.
add_executable(protected_reduce_guard_test guard_test.cpp)
target_include_directories(
  protected_reduce_guard_test
  SYSTEM PRIVATE ${IGL_INCLUDE_DIR} ${EIGEN3_INCLUDE_DIR}
)
target_compile_options(protected_reduce_guard_test PRIVATE -O2 -DNDEBUG)
target_link_libraries(protected_reduce_guard_test PRIVATE Threads::Threads)

install(TARGETS protected_reduce RUNTIME DESTINATION bin)
