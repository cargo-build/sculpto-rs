# Content identity of the provided libigl include tree.
#
# The build embeds this value; the executable cannot claim a revision it did not see.
# Method: every .h/.hpp/.cpp file under <IGL_INCLUDE_DIR>/igl is listed by relative
# path, sorted, paired with its own SHA256, and the resulting text manifest is hashed
# once more. The qualified upstream revision is kept beside it as a *reference* the
# tree was qualified against, never as a measured fact.
#
# Include from CMakeLists (sets IGL_TREE_SHA256 / IGL_TREE_FILE_COUNT) or run in
# script mode: cmake -DIGL_INCLUDE_DIR=<dir> -P igl_identity.cmake

if(NOT IGL_INCLUDE_DIR)
  message(FATAL_ERROR "IGL_INCLUDE_DIR is required")
endif()

file(
  GLOB_RECURSE IGL_TREE_PATHS
  LIST_DIRECTORIES false
  "${IGL_INCLUDE_DIR}/igl/*.h"
  "${IGL_INCLUDE_DIR}/igl/*.hpp"
  "${IGL_INCLUDE_DIR}/igl/*.cpp"
)
list(LENGTH IGL_TREE_PATHS IGL_TREE_FILE_COUNT)
if(IGL_TREE_FILE_COUNT EQUAL 0)
  message(FATAL_ERROR "no libigl headers or sources found under ${IGL_INCLUDE_DIR}/igl")
endif()
list(SORT IGL_TREE_PATHS)

set(IGL_TREE_MANIFEST "")
foreach(path IN LISTS IGL_TREE_PATHS)
  file(RELATIVE_PATH relative "${IGL_INCLUDE_DIR}" "${path}")
  file(SHA256 "${path}" file_digest)
  string(APPEND IGL_TREE_MANIFEST "${relative} ${file_digest}\n")
endforeach()
string(SHA256 IGL_TREE_SHA256 "${IGL_TREE_MANIFEST}")

if(CMAKE_SCRIPT_MODE_FILE)
  message("IGL_TREE_SHA256=${IGL_TREE_SHA256} IGL_TREE_FILE_COUNT=${IGL_TREE_FILE_COUNT}")
endif()
