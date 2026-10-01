#include <catch2/catch.hpp>
#include <stdexcept>
#include <string>

#include "angzarr/router/router.h"

using angzarr::router::CheckAbiVersion;
using angzarr::router::Router;
namespace ffi = angzarr::router::ffi;

TEST_CASE("a matching ABI version is accepted", "[abi]") {
  REQUIRE_NOTHROW(CheckAbiVersion(ffi::kAbiVersion));
}

TEST_CASE("an ABI version mismatch is refused naming expected and actual", "[abi]") {
  try {
    CheckAbiVersion(ffi::kAbiVersion + 1);
    FAIL("a mismatching ABI version was accepted");
  } catch (const std::runtime_error& e) {
    const std::string msg = e.what();
    REQUIRE(msg.find("expected 3") != std::string::npos);
    REQUIRE(msg.find("got 4") != std::string::npos);
  }
}

TEST_CASE("the linked router-ffi reports the binding's ABI version", "[abi]") {
  REQUIRE(ffi::kAbiVersion == 3);
  REQUIRE(Router::AbiVersion() == ffi::kAbiVersion);
}
