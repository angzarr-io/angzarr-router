#pragma once

// UUID v5 (RFC 4122, SHA-1 name-based) in the OID namespace: the conformance
// roots of labels, byte-identical across every language's step layer.

#include <array>
#include <cstdint>
#include <string>

namespace angzarr::conformance {

namespace detail {

inline uint32_t Rotl(uint32_t x, int n) { return (x << n) | (x >> (32 - n)); }

// The SHA-1 digest of data.
inline std::array<uint8_t, 20> Sha1(const std::string& data) {
  uint32_t h[5] = {0x67452301, 0xEFCDAB89, 0x98BADCFE, 0x10325476, 0xC3D2E1F0};
  std::string msg = data;
  const uint64_t bit_len = static_cast<uint64_t>(data.size()) * 8;
  msg.push_back(static_cast<char>(0x80));
  while (msg.size() % 64 != 56) msg.push_back('\0');
  for (int i = 7; i >= 0; --i) msg.push_back(static_cast<char>((bit_len >> (i * 8)) & 0xff));

  for (size_t chunk = 0; chunk < msg.size(); chunk += 64) {
    uint32_t w[80];
    for (int i = 0; i < 16; ++i) {
      const auto* p = reinterpret_cast<const uint8_t*>(msg.data() + chunk + i * 4);
      w[i] = (uint32_t{p[0]} << 24) | (uint32_t{p[1]} << 16) | (uint32_t{p[2]} << 8) | p[3];
    }
    for (int i = 16; i < 80; ++i) w[i] = Rotl(w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16], 1);
    uint32_t a = h[0], b = h[1], c = h[2], d = h[3], e = h[4];
    for (int i = 0; i < 80; ++i) {
      uint32_t f, k;
      if (i < 20) {
        f = (b & c) | (~b & d);
        k = 0x5A827999;
      } else if (i < 40) {
        f = b ^ c ^ d;
        k = 0x6ED9EBA1;
      } else if (i < 60) {
        f = (b & c) | (b & d) | (c & d);
        k = 0x8F1BBCDC;
      } else {
        f = b ^ c ^ d;
        k = 0xCA62C1D6;
      }
      const uint32_t t = Rotl(a, 5) + f + e + k + w[i];
      e = d;
      d = c;
      c = Rotl(b, 30);
      b = a;
      a = t;
    }
    h[0] += a;
    h[1] += b;
    h[2] += c;
    h[3] += d;
    h[4] += e;
  }
  std::array<uint8_t, 20> out{};
  for (int i = 0; i < 5; ++i) {
    for (int j = 0; j < 4; ++j) out[i * 4 + j] = static_cast<uint8_t>(h[i] >> (24 - j * 8));
  }
  return out;
}

}  // namespace detail

// The 16 root bytes for label: UUID v5 of label in the OID namespace
// (6ba7b812-9dad-11d1-80b4-00c04fd430c8).
inline std::string RootOf(const std::string& label) {
  static const std::string kNamespaceOid = {'\x6b', '\xa7', '\xb8', '\x12', '\x9d', '\xad',
                                            '\x11', '\xd1', '\x80', '\xb4', '\x00', '\xc0',
                                            '\x4f', '\xd4', '\x30', '\xc8'};
  const auto digest = detail::Sha1(kNamespaceOid + label);
  std::string root(reinterpret_cast<const char*>(digest.data()), 16);
  root[6] = static_cast<char>((static_cast<uint8_t>(root[6]) & 0x0f) | 0x50);
  root[8] = static_cast<char>((static_cast<uint8_t>(root[8]) & 0x3f) | 0x80);
  return root;
}

}  // namespace angzarr::conformance
