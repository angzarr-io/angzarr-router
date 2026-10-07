package io.angzarr.router.conformance;

import java.nio.ByteBuffer;
import java.nio.charset.StandardCharsets;
import java.security.MessageDigest;
import java.security.NoSuchAlgorithmException;
import java.util.Arrays;
import java.util.UUID;

/** RFC 4122 name-based UUIDs (version 5, SHA-1) in the OID namespace. */
public final class Uuid5 {
  private Uuid5() {}

  /** The RFC 4122 NAMESPACE_OID. */
  static final UUID NAMESPACE_OID = UUID.fromString("6ba7b812-9dad-11d1-80b4-00c04fd430c8");

  /** The 16 bytes of uuid5(NAMESPACE_OID, label). */
  public static byte[] oid(String label) {
    MessageDigest sha1;
    try {
      sha1 = MessageDigest.getInstance("SHA-1");
    } catch (NoSuchAlgorithmException e) {
      throw new IllegalStateException("SHA-1 unavailable", e);
    }
    ByteBuffer ns = ByteBuffer.allocate(16);
    ns.putLong(NAMESPACE_OID.getMostSignificantBits());
    ns.putLong(NAMESPACE_OID.getLeastSignificantBits());
    sha1.update(ns.array());
    sha1.update(label.getBytes(StandardCharsets.UTF_8));
    byte[] bytes = Arrays.copyOf(sha1.digest(), 16);
    bytes[6] = (byte) ((bytes[6] & 0x0f) | 0x50);
    bytes[8] = (byte) ((bytes[8] & 0x3f) | 0x80);
    return bytes;
  }
}
