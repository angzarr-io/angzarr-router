package io.angzarr.router;

import static org.junit.jupiter.api.Assertions.assertDoesNotThrow;
import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;

import org.junit.jupiter.api.Test;

/** The binding refuses a router-ffi library whose ABI version drifted. */
class AbiVersionTest {

  @Test
  void theLoadedLibraryReportsTheExpectedVersion() {
    assertEquals(Ffi.EXPECTED_ABI_VERSION, Router.abiVersion());
    assertEquals(1, Ffi.EXPECTED_ABI_VERSION);
  }

  @Test
  void aMismatchedVersionIsRefusedNamingBothVersions() {
    IllegalStateException e =
        assertThrows(IllegalStateException.class, () -> Ffi.checkAbiVersion(2));
    assertTrue(e.getMessage().contains("expected 1"), e.getMessage());
    assertTrue(e.getMessage().contains("got 2"), e.getMessage());
  }

  @Test
  void theExpectedVersionIsAccepted() {
    assertDoesNotThrow(() -> Ffi.checkAbiVersion(1));
  }
}
