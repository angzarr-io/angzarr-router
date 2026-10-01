package io.angzarr.router.conformance;

import static org.junit.jupiter.api.Assertions.assertEquals;

import java.nio.ByteBuffer;
import java.util.UUID;
import org.junit.jupiter.api.Test;

/** Entity roots derive from labels exactly as Python's uuid.uuid5(NAMESPACE_OID, label). */
class Uuid5Test {

  private static UUID of(byte[] bytes) {
    ByteBuffer b = ByteBuffer.wrap(bytes);
    return new UUID(b.getLong(), b.getLong());
  }

  @Test
  void matchesTheReferenceVectors() {
    assertEquals(
        UUID.fromString("280aa180-e8cc-5bc1-9758-6aeeda5ec0b0"), of(Uuid5.oid("ledger-1")));
    assertEquals(UUID.fromString("eba6a19b-488f-5e10-97a5-a34a92553679"), of(Uuid5.oid("table-1")));
  }
}
