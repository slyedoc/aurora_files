cmake_minimum_required(VERSION 3.20)
project(mhdna CXX)

# RigLogicLib from an Unreal checkout: the DNA 2.5 reader and the RigLogic evaluator that UE
# itself runs. Only its plain-C++ core is compiled; the UE module glue is left out.
set(RIGLOGIC_DIR "/mnt/code/f/UnrealEngine/Engine/Plugins/Animation/RigLogic/Source/RigLogicLib"
    CACHE PATH "UE RigLogicLib source directory")

set(CMAKE_CXX_STANDARD 17)
set(CMAKE_CXX_STANDARD_REQUIRED ON)
if(NOT CMAKE_BUILD_TYPE)
    set(CMAKE_BUILD_TYPE Release)
endif()

file(GLOB_RECURSE RL_SOURCES
    ${RIGLOGIC_DIR}/Private/dna/*.cpp
    ${RIGLOGIC_DIR}/Private/pma/*.cpp
    ${RIGLOGIC_DIR}/Private/riglogic/*.cpp
    ${RIGLOGIC_DIR}/Private/status/*.cpp
    ${RIGLOGIC_DIR}/Private/trio/*.cpp)

add_library(riglogic STATIC ${RL_SOURCES})
target_include_directories(riglogic PUBLIC ${RIGLOGIC_DIR}/Public PRIVATE ${RIGLOGIC_DIR}/Private)
# The same defines UE's RigLogicLib.Build.cs gives an editor build on Linux.
target_compile_definitions(riglogic PUBLIC RL_BUILD_WITH_ZYX_ROTATION_ORDER=1 RL_AUTODETECT_SSE=1
    RL_AUTODETECT_HALF_FLOATS=1 TRIO_MMAP_AVAILABLE=1 TRIO_MREMAP_AVAILABLE=1)
target_compile_options(riglogic PRIVATE -msse4.1 -w)

add_executable(mhdna mhdna.cpp)
target_link_libraries(mhdna PRIVATE riglogic)
