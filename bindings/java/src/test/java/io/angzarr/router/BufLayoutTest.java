package io.angzarr.router;

import static org.junit.jupiter.api.Assertions.assertEquals;

import java.lang.foreign.Linker;
import java.lang.foreign.MemoryLayout;
import java.lang.foreign.ValueLayout;
import org.junit.jupiter.api.Test;

/** AngzarrBuf { uint8_t *data; size_t len } is laid out with the platform's size_t. */
class BufLayoutTest {

  private static final MemoryLayout PLATFORM_SIZE_T =
      Linker.nativeLinker().canonicalLayouts().get("size_t");

  @Test
  void sizeTIsThePlatformCanonicalLayout() {
    assertEquals(PLATFORM_SIZE_T.byteSize(), Ffi.SIZE_T.byteSize());
    assertEquals(PLATFORM_SIZE_T.byteAlignment(), Ffi.SIZE_T.byteAlignment());
  }

  @Test
  void bufLenFollowsDataAndHasTheSizeTWidth() {
    MemoryLayout len = Ffi.ANGZARR_BUF.select(MemoryLayout.PathElement.groupElement("len"));
    assertEquals(PLATFORM_SIZE_T.byteSize(), len.byteSize());
    assertEquals(
        Ffi.ANGZARR_BUF.byteOffset(MemoryLayout.PathElement.groupElement("len")),
        Ffi.BUF_LEN_OFFSET);
    assertEquals(ValueLayout.ADDRESS.byteSize(), Ffi.BUF_LEN_OFFSET);
    assertEquals(
        ValueLayout.ADDRESS.byteSize() + PLATFORM_SIZE_T.byteSize(), Ffi.ANGZARR_BUF.byteSize());
  }
}
