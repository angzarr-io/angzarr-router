cmake_minimum_required(VERSION 3.20)
project(angzarr_router_cpp CXX)

# The C++ binding links the router-ffi STATICLIB directly (no runtime .so). The
# generated protobuf C++ (framework, abi, google/rpc, test) + the angzarr wiring
# headers live under gen/ (produced by `just cpp-binding-gen`, never committed).

set(CMAKE_CXX_STANDARD 20)
set(CMAKE_CXX_STANDARD_REQUIRED ON)

find_package(Protobuf REQUIRED)

set(ANGZARR_GEN_DIR "${CMAKE_CURRENT_SOURCE_DIR}/gen" CACHE PATH "generated protobuf + wiring")
set(ANGZARR_STATICLIB "${CMAKE_CURRENT_SOURCE_DIR}/../../target/debug/libangzarr_router_ffi.a"
    CACHE FILEPATH "router-ffi staticlib")

# Generated protobuf C++ runtime types (compiled once; warnings silenced).
file(GLOB_RECURSE GEN_SRCS CONFIGURE_DEPENDS "${ANGZARR_GEN_DIR}/*.pb.cc")
add_library(angzarr_gen STATIC ${GEN_SRCS})
target_include_directories(angzarr_gen PUBLIC "${ANGZARR_GEN_DIR}")
target_link_libraries(angzarr_gen PUBLIC protobuf::libprotobuf)
target_compile_options(angzarr_gen PRIVATE -w)

# The header-only binding + generated wiring headers, plus the native staticlib.
# A Rust staticlib needs the std system deps (pthread/dl/m) at link time.
add_library(angzarr_router INTERFACE)
target_include_directories(angzarr_router INTERFACE
    "${CMAKE_CURRENT_SOURCE_DIR}/include" "${ANGZARR_GEN_DIR}")
target_link_libraries(angzarr_router INTERFACE
    angzarr_gen "${ANGZARR_STATICLIB}" pthread dl m)

# Smoke: proves the binding + generated wiring compile, instantiate, link, and
# round-trip one dispatch through the staticlib.
add_executable(smoke "${CMAKE_CURRENT_SOURCE_DIR}/src/smoke.cpp")
target_link_libraries(smoke PRIVATE angzarr_router)

enable_testing()
add_test(NAME smoke COMMAND smoke)

# Conformance: the shared behavioral features (read at runtime from the canonical
# conformance/features via ANGZARR_FEATURES_DIR) run through a small Gherkin
# feature-runner on Catch2.
find_package(Catch2 2 REQUIRED)
file(GLOB CONFORMANCE_SRCS CONFIGURE_DEPENDS "${CMAKE_CURRENT_SOURCE_DIR}/conformance/*.cpp")
add_executable(conformance ${CONFORMANCE_SRCS})
target_include_directories(conformance PRIVATE "${CMAKE_CURRENT_SOURCE_DIR}/conformance")
target_link_libraries(conformance PRIVATE angzarr_router Catch2::Catch2)
target_compile_definitions(conformance PRIVATE
    ANGZARR_FEATURES_DIR="${CMAKE_CURRENT_SOURCE_DIR}/../../conformance/features")
add_test(NAME conformance COMMAND conformance)
