using System;
using System.Reflection;
using System.Runtime.CompilerServices;
using System.Runtime.InteropServices;
using System.Text;

namespace Angzarr.Router;

/// <summary>AngzarrBuf { *mut u8 data; usize len } — the byte buffer crossing
/// the C ABI.</summary>
[StructLayout(LayoutKind.Sequential)]
internal struct AngzarrBuf
{
    public IntPtr Data;
    public nuint Len;
}

/// <summary>
/// The raw C-ABI layer over the router-ffi cdylib, via P/Invoke. Holds the 11
/// exported downcalls, the <see cref="AngzarrBuf"/> layout, and the single
/// <c>[UnmanagedCallersOnly]</c> upcall trampoline the core calls for every host
/// callback.
///
/// <para>Memory ownership is symmetric, copy-at-the-boundary: a callback fills
/// the router-allocated <c>out</c> (via <c>angzarr_buf_alloc</c>); a dispatch
/// response is router-allocated and released here (<c>angzarr_buf_release</c>).
/// The trampoline catches every exception and codes it — an exception never
/// unwinds across the boundary. The per-dispatch <see cref="Session"/> is parked
/// in a <see cref="GCHandle"/> whose <see cref="IntPtr"/> the core carries as
/// <c>host_ctx</c>.</para>
/// </summary>
internal static unsafe class Ffi
{
    internal const int StatusOk = 0;
    internal const int StatusOkEmpty = 1;

    private const string Lib = "angzarr_router_ffi";
    private const string LibEnv = "ANGZARR_ROUTER_LIB";

    // The single host-callback gateway, shared across every registration. It is
    // stateless: it reaches the session via host_ctx and the registry via the
    // session's router, so one global stub suffices.
    private static readonly IntPtr Callback = (IntPtr)
        (delegate* unmanaged[Cdecl]<
            IntPtr,
            ulong,
            byte*,
            nuint,
            byte*,
            nuint,
            byte*,
            nuint,
            AngzarrBuf*,
            int>)
            &Trampoline;

    static Ffi()
    {
        NativeLibrary.SetDllImportResolver(typeof(Ffi).Assembly, Resolve);
        var v = angzarr_abi_version();
        if (v != 1)
        {
            throw new InvalidOperationException($"router-ffi ABI version {v} != 1");
        }
    }

    private static IntPtr Resolve(
        string libraryName,
        Assembly assembly,
        DllImportSearchPath? searchPath
    )
    {
        if (libraryName != Lib)
        {
            return IntPtr.Zero;
        }
        var path = Environment.GetEnvironmentVariable(LibEnv);
        if (string.IsNullOrEmpty(path))
        {
            throw new InvalidOperationException(
                $"router-ffi cdylib path not set — set {LibEnv} to libangzarr_router_ffi.so"
            );
        }
        return NativeLibrary.Load(path);
    }

    // --- the 11 downcalls ---------------------------------------------------

    [DllImport(Lib)]
    private static extern uint angzarr_abi_version();

    [DllImport(Lib)]
    private static extern IntPtr angzarr_buf_alloc(nuint len);

    [DllImport(Lib)]
    private static extern void angzarr_buf_release(IntPtr ptr, nuint len);

    [DllImport(Lib)]
    private static extern IntPtr angzarr_router_new();

    [DllImport(Lib)]
    private static extern void angzarr_router_free(IntPtr r);

    [DllImport(Lib)]
    private static extern int angzarr_router_register_aggregate(
        IntPtr r,
        byte* descriptor,
        nuint len,
        IntPtr cb
    );

    [DllImport(Lib)]
    private static extern int angzarr_router_register_projector(
        IntPtr r,
        byte* descriptor,
        nuint len,
        IntPtr cb
    );

    [DllImport(Lib)]
    private static extern int angzarr_router_register_saga(
        IntPtr r,
        byte* descriptor,
        nuint len,
        IntPtr cb
    );

    [DllImport(Lib)]
    private static extern int angzarr_router_register_process_manager(
        IntPtr r,
        byte* descriptor,
        nuint len,
        IntPtr cb
    );

    [DllImport(Lib)]
    private static extern int angzarr_router_dispatch(
        IntPtr r,
        IntPtr hostCtx,
        byte* request,
        nuint len,
        AngzarrBuf* outBuf
    );

    [DllImport(Lib)]
    private static extern int angzarr_router_dispatch_projector(
        IntPtr r,
        IntPtr hostCtx,
        byte* request,
        nuint len,
        AngzarrBuf* outBuf
    );

    [DllImport(Lib)]
    private static extern int angzarr_router_dispatch_saga(
        IntPtr r,
        IntPtr hostCtx,
        byte* request,
        nuint len,
        AngzarrBuf* outBuf
    );

    [DllImport(Lib)]
    private static extern int angzarr_router_dispatch_process_manager(
        IntPtr r,
        IntPtr hostCtx,
        byte* request,
        nuint len,
        AngzarrBuf* outBuf
    );

    // --- lifecycle ----------------------------------------------------------

    internal static IntPtr RouterNew() => angzarr_router_new();

    internal static void RouterFree(IntPtr r) => angzarr_router_free(r);

    // --- registration -------------------------------------------------------

    internal static int RegisterAggregate(IntPtr r, byte[] descriptor)
    {
        fixed (byte* p = descriptor)
        {
            return angzarr_router_register_aggregate(r, p, (nuint)descriptor.Length, Callback);
        }
    }

    internal static int RegisterProjector(IntPtr r, byte[] descriptor)
    {
        fixed (byte* p = descriptor)
        {
            return angzarr_router_register_projector(r, p, (nuint)descriptor.Length, Callback);
        }
    }

    internal static int RegisterSaga(IntPtr r, byte[] descriptor)
    {
        fixed (byte* p = descriptor)
        {
            return angzarr_router_register_saga(r, p, (nuint)descriptor.Length, Callback);
        }
    }

    internal static int RegisterProcessManager(IntPtr r, byte[] descriptor)
    {
        fixed (byte* p = descriptor)
        {
            return angzarr_router_register_process_manager(
                r,
                p,
                (nuint)descriptor.Length,
                Callback
            );
        }
    }

    // --- dispatch -----------------------------------------------------------

    /// <summary>One dispatch downcall's outcome: response bytes (possibly null)
    /// + status.</summary>
    internal readonly record struct Dispatched(byte[]? Response, int Status);

    internal static Dispatched Dispatch(IntPtr r, IntPtr hostCtx, byte[] request)
    {
        AngzarrBuf outBuf = default;
        int ret;
        fixed (byte* p = request)
        {
            ret = angzarr_router_dispatch(r, hostCtx, p, (nuint)request.Length, &outBuf);
        }
        return new Dispatched(ConsumeOut(&outBuf), ret);
    }

    internal static Dispatched DispatchProjector(IntPtr r, IntPtr hostCtx, byte[] request)
    {
        AngzarrBuf outBuf = default;
        int ret;
        fixed (byte* p = request)
        {
            ret = angzarr_router_dispatch_projector(r, hostCtx, p, (nuint)request.Length, &outBuf);
        }
        return new Dispatched(ConsumeOut(&outBuf), ret);
    }

    internal static Dispatched DispatchSaga(IntPtr r, IntPtr hostCtx, byte[] request)
    {
        AngzarrBuf outBuf = default;
        int ret;
        fixed (byte* p = request)
        {
            ret = angzarr_router_dispatch_saga(r, hostCtx, p, (nuint)request.Length, &outBuf);
        }
        return new Dispatched(ConsumeOut(&outBuf), ret);
    }

    internal static Dispatched DispatchProcessManager(IntPtr r, IntPtr hostCtx, byte[] request)
    {
        AngzarrBuf outBuf = default;
        int ret;
        fixed (byte* p = request)
        {
            ret = angzarr_router_dispatch_process_manager(
                r,
                hostCtx,
                p,
                (nuint)request.Length,
                &outBuf
            );
        }
        return new Dispatched(ConsumeOut(&outBuf), ret);
    }

    // --- the trampoline (called by the core, on the dispatching thread) -----

    [UnmanagedCallersOnly(CallConvs = new[] { typeof(CallConvCdecl) })]
    private static int Trampoline(
        IntPtr hostCtx,
        ulong callbackId,
        byte* typeUrl,
        nuint typeUrlLen,
        byte* payload,
        nuint payloadLen,
        byte* aux,
        nuint auxLen,
        AngzarrBuf* outBuf
    )
    {
        try
        {
            var session =
                hostCtx == IntPtr.Zero ? null : GCHandle.FromIntPtr(hostCtx).Target as Session;
            var invoker = session?.Router.InvokerFor(callbackId);
            if (invoker == null)
            {
                return Fail(
                    outBuf,
                    CodedError.Unhandled($"no host callback registered for id {callbackId}")
                );
            }
            InvokerResult result;
            try
            {
                result = invoker(
                    session!,
                    ReadString(typeUrl, typeUrlLen),
                    ReadBytes(payload, payloadLen),
                    ReadBytes(aux, auxLen)
                );
            }
            catch (Exception handlerError)
            {
                result = Statuses.ErrorResult(handlerError);
            }
            WriteOut(outBuf, result.Response);
            return result.Status;
        }
        catch (Exception fatal)
        {
            return Fail(outBuf, CodedError.Unhandled($"csharp callback gateway failed: {fatal}"));
        }
    }

    private static int Fail(AngzarrBuf* outBuf, CodedError err)
    {
        var result = Statuses.ErrorResult(err);
        WriteOut(outBuf, result.Response);
        return result.Status;
    }

    // --- buffer marshalling -------------------------------------------------

    private static byte[] ReadBytes(byte* ptr, nuint len)
    {
        if (ptr == null || len == 0)
        {
            return Array.Empty<byte>();
        }
        return new ReadOnlySpan<byte>(ptr, (int)len).ToArray();
    }

    private static string ReadString(byte* ptr, nuint len)
    {
        if (ptr == null || len == 0)
        {
            return "";
        }
        return Encoding.UTF8.GetString(ptr, (int)len);
    }

    /// <summary>Writes host bytes into a router-allocated out buffer (the host
    /// fills out via the router's allocator; the router consumes and frees it).
    /// An empty payload leaves out null/zero.</summary>
    private static void WriteOut(AngzarrBuf* outBuf, byte[]? bytes)
    {
        if (outBuf == null)
        {
            return;
        }
        if (bytes == null || bytes.Length == 0)
        {
            outBuf->Data = IntPtr.Zero;
            outBuf->Len = 0;
            return;
        }
        var data = angzarr_buf_alloc((nuint)bytes.Length);
        Marshal.Copy(bytes, 0, data, bytes.Length);
        outBuf->Data = data;
        outBuf->Len = (nuint)bytes.Length;
    }

    /// <summary>Copies a router-allocated out buffer into managed memory and
    /// releases it (the dispatch out is router-owned).</summary>
    private static byte[] ConsumeOut(AngzarrBuf* outBuf)
    {
        if (outBuf->Data == IntPtr.Zero || outBuf->Len == 0)
        {
            return Array.Empty<byte>();
        }
        var len = (int)outBuf->Len;
        var bytes = new byte[len];
        Marshal.Copy(outBuf->Data, bytes, 0, len);
        angzarr_buf_release(outBuf->Data, outBuf->Len);
        return bytes;
    }
}
