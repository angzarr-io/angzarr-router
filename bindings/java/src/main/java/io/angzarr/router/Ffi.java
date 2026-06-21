package io.angzarr.router;

import java.lang.foreign.Arena;
import java.lang.foreign.FunctionDescriptor;
import java.lang.foreign.Linker;
import java.lang.foreign.MemoryLayout;
import java.lang.foreign.MemorySegment;
import java.lang.foreign.SymbolLookup;
import java.lang.foreign.ValueLayout;
import java.lang.invoke.MethodHandle;
import java.lang.invoke.MethodHandles;
import java.lang.invoke.MethodType;
import java.nio.charset.StandardCharsets;
import java.nio.file.Path;
import java.util.concurrent.ConcurrentHashMap;
import java.util.concurrent.atomic.AtomicLong;

/**
 * The raw C-ABI layer over the router-ffi cdylib, via Panama/FFM. Holds the
 * downcall handles for the 11 exported functions, the {@code AngzarrBuf} layout,
 * and the single upcall trampoline the core calls for every host callback.
 *
 * <p>Memory ownership is symmetric, copy-at-the-boundary: a callback fills the
 * router-allocated {@code out} (via {@code angzarr_buf_alloc}); a dispatch
 * response is router-allocated and released here ({@code angzarr_buf_release}).
 * The trampoline catches every throwable and codes it — an exception never
 * unwinds across the boundary.
 */
final class Ffi {
  private Ffi() {}

  static final int STATUS_OK = 0;
  static final int STATUS_OK_EMPTY = 1;

  private static final String LIB_PROP = "angzarr.router.lib";
  private static final String LIB_ENV = "ANGZARR_ROUTER_LIB";

  private static final Linker LINKER = Linker.nativeLinker();
  private static final SymbolLookup LIB = loadLibrary();

  // AngzarrBuf { *mut u8 data; usize len } — data at 0, len at 8 (64-bit).
  private static final MemoryLayout ANGZARR_BUF =
      MemoryLayout.structLayout(ValueLayout.ADDRESS.withName("data"), ValueLayout.JAVA_LONG.withName("len"));
  private static final long BUF_LEN_OFFSET = 8;

  private static final MethodHandle ABI_VERSION = down("angzarr_abi_version", FunctionDescriptor.of(ValueLayout.JAVA_INT));
  private static final MethodHandle BUF_ALLOC = down("angzarr_buf_alloc", FunctionDescriptor.of(ValueLayout.ADDRESS, ValueLayout.JAVA_LONG));
  private static final MethodHandle BUF_RELEASE = down("angzarr_buf_release", FunctionDescriptor.ofVoid(ValueLayout.ADDRESS, ValueLayout.JAVA_LONG));
  private static final MethodHandle ROUTER_NEW = down("angzarr_router_new", FunctionDescriptor.of(ValueLayout.ADDRESS));
  private static final MethodHandle ROUTER_FREE = down("angzarr_router_free", FunctionDescriptor.ofVoid(ValueLayout.ADDRESS));

  // register: (router, descriptor, descriptor_len, cb) -> i32
  private static final FunctionDescriptor REGISTER_DESC =
      FunctionDescriptor.of(ValueLayout.JAVA_INT, ValueLayout.ADDRESS, ValueLayout.ADDRESS, ValueLayout.JAVA_LONG, ValueLayout.ADDRESS);
  private static final MethodHandle REGISTER_AGGREGATE = down("angzarr_router_register_aggregate", REGISTER_DESC);
  private static final MethodHandle REGISTER_PROJECTOR = down("angzarr_router_register_projector", REGISTER_DESC);
  private static final MethodHandle REGISTER_SAGA = down("angzarr_router_register_saga", REGISTER_DESC);
  private static final MethodHandle REGISTER_PROCESS_MANAGER = down("angzarr_router_register_process_manager", REGISTER_DESC);

  // dispatch: (router, host_ctx, request, request_len, out) -> i32
  private static final FunctionDescriptor DISPATCH_DESC =
      FunctionDescriptor.of(ValueLayout.JAVA_INT, ValueLayout.ADDRESS, ValueLayout.ADDRESS, ValueLayout.ADDRESS, ValueLayout.JAVA_LONG, ValueLayout.ADDRESS);
  private static final MethodHandle DISPATCH = down("angzarr_router_dispatch", DISPATCH_DESC);
  private static final MethodHandle DISPATCH_PROJECTOR = down("angzarr_router_dispatch_projector", DISPATCH_DESC);
  private static final MethodHandle DISPATCH_SAGA = down("angzarr_router_dispatch_saga", DISPATCH_DESC);
  private static final MethodHandle DISPATCH_PROCESS_MANAGER = down("angzarr_router_dispatch_process_manager", DISPATCH_DESC);

  // The single host-callback gateway, shared across every registration. It is
  // stateless: it reaches the session via host_ctx and the registry via the
  // session's router, so one global stub suffices.
  private static final MemorySegment CALLBACK = upcallStub();

  // host_ctx parking: FFM cannot stash a Java object in a C void*, so a
  // per-dispatch long id keys the session and is passed as the host_ctx pointer.
  private static final ConcurrentHashMap<Long, Session> SESSIONS = new ConcurrentHashMap<>();
  private static final AtomicLong NEXT_SESSION = new AtomicLong(1);

  static {
    int v;
    try {
      v = (int) ABI_VERSION.invokeExact();
    } catch (Throwable t) {
      throw new ExceptionInInitializerError(t);
    }
    if (v != 1) {
      throw new IllegalStateException("router-ffi ABI version " + v + " != 1");
    }
  }

  private static SymbolLookup loadLibrary() {
    String p = System.getProperty(LIB_PROP);
    if (p == null || p.isBlank()) {
      p = System.getenv(LIB_ENV);
    }
    if (p == null || p.isBlank()) {
      throw new IllegalStateException(
          "router-ffi cdylib path not set — pass -D" + LIB_PROP + " or " + LIB_ENV);
    }
    return SymbolLookup.libraryLookup(Path.of(p), Arena.global());
  }

  private static MethodHandle down(String name, FunctionDescriptor desc) {
    MemorySegment sym =
        LIB.find(name).orElseThrow(() -> new IllegalStateException("missing FFI symbol: " + name));
    return LINKER.downcallHandle(sym, desc);
  }

  private static MemorySegment upcallStub() {
    try {
      MethodHandle handle =
          MethodHandles.lookup()
              .findStatic(
                  Ffi.class,
                  "trampoline",
                  MethodType.methodType(
                      int.class,
                      MemorySegment.class, // host_ctx
                      long.class, // callback_id
                      MemorySegment.class, // type_url
                      long.class, // type_url_len
                      MemorySegment.class, // payload
                      long.class, // payload_len
                      MemorySegment.class, // aux
                      long.class, // aux_len
                      MemorySegment.class)); // out
      FunctionDescriptor desc =
          FunctionDescriptor.of(
              ValueLayout.JAVA_INT,
              ValueLayout.ADDRESS,
              ValueLayout.JAVA_LONG,
              ValueLayout.ADDRESS,
              ValueLayout.JAVA_LONG,
              ValueLayout.ADDRESS,
              ValueLayout.JAVA_LONG,
              ValueLayout.ADDRESS,
              ValueLayout.JAVA_LONG,
              ValueLayout.ADDRESS);
      return LINKER.upcallStub(handle, desc, Arena.global());
    } catch (NoSuchMethodException | IllegalAccessException e) {
      throw new ExceptionInInitializerError(e);
    }
  }

  // --- session lifecycle (Router opens one per dispatch) ------------------

  static long openSession(Session session) {
    long id = NEXT_SESSION.getAndIncrement();
    SESSIONS.put(id, session);
    return id;
  }

  static void closeSession(long id) {
    SESSIONS.remove(id);
  }

  // --- lifecycle + register + dispatch ------------------------------------

  static MemorySegment routerNew() {
    try {
      return (MemorySegment) ROUTER_NEW.invokeExact();
    } catch (Throwable t) {
      throw rethrow(t);
    }
  }

  static void routerFree(MemorySegment router) {
    try {
      ROUTER_FREE.invokeExact(router);
    } catch (Throwable t) {
      throw rethrow(t);
    }
  }

  static int registerAggregate(MemorySegment router, byte[] descriptor) {
    return register(REGISTER_AGGREGATE, router, descriptor);
  }

  static int registerProjector(MemorySegment router, byte[] descriptor) {
    return register(REGISTER_PROJECTOR, router, descriptor);
  }

  static int registerSaga(MemorySegment router, byte[] descriptor) {
    return register(REGISTER_SAGA, router, descriptor);
  }

  static int registerProcessManager(MemorySegment router, byte[] descriptor) {
    return register(REGISTER_PROCESS_MANAGER, router, descriptor);
  }

  private static int register(MethodHandle handle, MemorySegment router, byte[] descriptor) {
    try (Arena arena = Arena.ofConfined()) {
      MemorySegment desc = toSegment(arena, descriptor);
      return (int) handle.invokeExact(router, desc, (long) descriptor.length, CALLBACK);
    } catch (Throwable t) {
      throw rethrow(t);
    }
  }

  /** One dispatch downcall's outcome: response bytes (possibly null) + status. */
  record Dispatched(byte[] response, int status) {}

  static Dispatched dispatch(MemorySegment router, long sessionId, byte[] request) {
    return dispatch(DISPATCH, router, sessionId, request);
  }

  static Dispatched dispatchProjector(MemorySegment router, long sessionId, byte[] request) {
    return dispatch(DISPATCH_PROJECTOR, router, sessionId, request);
  }

  static Dispatched dispatchSaga(MemorySegment router, long sessionId, byte[] request) {
    return dispatch(DISPATCH_SAGA, router, sessionId, request);
  }

  static Dispatched dispatchProcessManager(MemorySegment router, long sessionId, byte[] request) {
    return dispatch(DISPATCH_PROCESS_MANAGER, router, sessionId, request);
  }

  private static Dispatched dispatch(
      MethodHandle handle, MemorySegment router, long sessionId, byte[] request) {
    try (Arena arena = Arena.ofConfined()) {
      MemorySegment req = toSegment(arena, request);
      MemorySegment out = arena.allocate(ANGZARR_BUF);
      MemorySegment hostCtx = MemorySegment.ofAddress(sessionId);
      int ret = (int) handle.invokeExact(router, hostCtx, req, (long) request.length, out);
      return new Dispatched(consumeOut(out), ret);
    } catch (Throwable t) {
      throw rethrow(t);
    }
  }

  // --- the trampoline (called by the core, on the dispatching thread) -----

  private static int trampoline(
      MemorySegment hostCtx,
      long callbackId,
      MemorySegment typeUrl,
      long typeUrlLen,
      MemorySegment payload,
      long payloadLen,
      MemorySegment aux,
      long auxLen,
      MemorySegment out) {
    try {
      Session session = SESSIONS.get(hostCtx.address());
      Invoker invoker = session == null ? null : session.router.invokerFor(callbackId);
      if (invoker == null) {
        return fail(
            out,
            CodedError.unhandled("no host callback registered for id " + callbackId));
      }
      Invoker.Result result;
      try {
        result =
            invoker.invoke(
                session,
                readString(typeUrl, typeUrlLen),
                readBytes(payload, payloadLen),
                readBytes(aux, auxLen));
      } catch (Throwable handlerError) {
        result = Statuses.errorResult(handlerError);
      }
      writeOut(out, result.response());
      return result.status();
    } catch (Throwable fatal) {
      return fail(out, CodedError.unhandled("java callback gateway failed: " + fatal));
    }
  }

  private static int fail(MemorySegment out, CodedError err) {
    Invoker.Result result = Statuses.errorResult(err);
    writeOut(out, result.response());
    return result.status();
  }

  // --- buffer marshalling -------------------------------------------------

  private static MemorySegment toSegment(Arena arena, byte[] bytes) {
    if (bytes.length == 0) {
      return MemorySegment.NULL;
    }
    MemorySegment seg = arena.allocate(bytes.length);
    MemorySegment.copy(bytes, 0, seg, ValueLayout.JAVA_BYTE, 0, bytes.length);
    return seg;
  }

  private static byte[] readBytes(MemorySegment seg, long len) {
    if (seg.address() == 0 || len == 0) {
      return new byte[0];
    }
    return seg.reinterpret(len).toArray(ValueLayout.JAVA_BYTE);
  }

  private static String readString(MemorySegment seg, long len) {
    return new String(readBytes(seg, len), StandardCharsets.UTF_8);
  }

  /** Writes host bytes into a router-allocated out buffer (the host fills out
   * via the router's allocator; the router consumes and frees it). An empty
   * payload leaves out null/zero. */
  private static void writeOut(MemorySegment out, byte[] bytes) {
    if (out.address() == 0) {
      return;
    }
    MemorySegment buf = out.reinterpret(ANGZARR_BUF.byteSize());
    if (bytes == null || bytes.length == 0) {
      buf.set(ValueLayout.ADDRESS, 0, MemorySegment.NULL);
      buf.set(ValueLayout.JAVA_LONG, BUF_LEN_OFFSET, 0L);
      return;
    }
    MemorySegment data;
    try {
      data = (MemorySegment) BUF_ALLOC.invokeExact((long) bytes.length);
    } catch (Throwable t) {
      throw rethrow(t);
    }
    MemorySegment.copy(bytes, 0, data.reinterpret(bytes.length), ValueLayout.JAVA_BYTE, 0, bytes.length);
    buf.set(ValueLayout.ADDRESS, 0, data);
    buf.set(ValueLayout.JAVA_LONG, BUF_LEN_OFFSET, (long) bytes.length);
  }

  /** Copies a router-allocated out buffer into Java memory and releases it (the
   * dispatch out is router-owned). */
  private static byte[] consumeOut(MemorySegment out) {
    MemorySegment data = out.get(ValueLayout.ADDRESS, 0);
    long len = out.get(ValueLayout.JAVA_LONG, BUF_LEN_OFFSET);
    if (data.address() == 0 || len == 0) {
      return new byte[0];
    }
    byte[] bytes = data.reinterpret(len).toArray(ValueLayout.JAVA_BYTE);
    try {
      BUF_RELEASE.invokeExact(data, len);
    } catch (Throwable t) {
      throw rethrow(t);
    }
    return bytes;
  }

  private static RuntimeException rethrow(Throwable t) {
    if (t instanceof RuntimeException re) {
      return re;
    }
    return new RuntimeException(t);
  }
}
