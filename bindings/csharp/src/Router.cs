using System;
using System.Collections.Concurrent;
using System.Collections.Generic;
using System.Runtime.InteropServices;
using System.Threading;
using Google.Protobuf;
using Google.Protobuf.WellKnownTypes;
using Abi = Io.Angzarr.Router.Ffi.V1;

namespace Angzarr.Router;

/// <summary>
/// The C# binding's router: wraps the native router plus the host-side callback
/// registry the core reaches through the single callback gateway. Register a
/// component (assigning callback ids to its thunks and handing the core a
/// serialized descriptor), then dispatch books/commands through it.
///
/// <para>The dispatch surfaces are generic in the component's state message, so
/// the generated wiring and handler thunks are statically typed. The one
/// unavoidable erasing cast — the FFI registry is keyed by an opaque
/// callback_id, not a type — lives in the invoker adapters below
/// (<c>(TState)session.EnsureState(...)</c>), guaranteed correct because the same
/// factory produces the state.</para>
/// </summary>
public sealed class Router : IDisposable
{
    private readonly IntPtr _ptr;
    private readonly ConcurrentDictionary<ulong, Invoker> _registry = new();
    private readonly object _lock = new();
    private long _nextId;

    public Router() => _ptr = Ffi.RouterNew();

    /// <summary>The ABI version the loaded cdylib reports (always 1 for a
    /// compatible lib).</summary>
    public static int AbiVersion() => 1;

    public void Dispose() => Ffi.RouterFree(_ptr);

    internal Invoker? InvokerFor(ulong callbackId) =>
        _registry.TryGetValue(callbackId, out var inv) ? inv : null;

    private ulong Assign(Invoker invoker)
    {
        var id = (ulong)Interlocked.Increment(ref _nextId);
        _registry[id] = invoker;
        return id;
    }

    // --- registration --------------------------------------------------------

    public void RegisterAggregate<TState>(AggregateDispatch<TState> d)
        where TState : class, IMessage
    {
        lock (_lock)
        {
            var factory = d.Rebuilder.Factory;
            var desc = new Abi.AggregateDescriptor { Name = d.Name, Domain = d.Domain };
            foreach (var (key, thunk) in d.Rebuilder.Appliers)
            {
                desc.Appliers.Add(CallbackEntry(key, Assign(ApplierInvoker(factory, thunk))));
            }
            if (d.Rebuilder.Snapshot != null)
            {
                desc.SnapshotCallbackId = Assign(ApplierInvoker(factory, d.Rebuilder.Snapshot));
            }
            foreach (var (key, thunk) in d.Commands)
            {
                desc.Commands.Add(CallbackEntry(key, Assign(CommandInvoker(factory, thunk))));
            }
            foreach (var (cmd, thunks) in d.Rejections)
            {
                var entry = new Abi.RejectionEntry { FqCommandType = cmd };
                foreach (var thunk in thunks)
                {
                    entry.CallbackIds.Add(Assign(RejectionInvoker(factory, thunk)));
                }
                desc.Rejections.Add(entry);
            }
            Check(Ffi.RegisterAggregate(_ptr, desc.ToByteArray()));
        }
    }

    public void RegisterProjector<TState>(ProjectorDispatch<TState> d)
        where TState : class, IMessage
    {
        lock (_lock)
        {
            var factory = d.Factory;
            var desc = new Abi.ProjectorDescriptor { Name = d.Name };
            desc.Domains.AddRange(d.Domains);
            foreach (var (key, thunk) in d.Events)
            {
                desc.Events.Add(CallbackEntry(key, Assign(ProjectorEventInvoker(factory, thunk))));
            }
            if (d.Unknown != null)
            {
                desc.UnknownCallbackId = Assign(ProjectorUnknownInvoker(d.Unknown));
            }
            if (d.FinishThunk != null)
            {
                desc.FinishCallbackId = Assign(ProjectorFinishInvoker(factory, d.FinishThunk));
            }
            Check(Ffi.RegisterProjector(_ptr, desc.ToByteArray()));
        }
    }

    public void RegisterSaga(SagaDispatch d)
    {
        lock (_lock)
        {
            var desc = new Abi.SagaDescriptor { Name = d.Name, InputDomain = d.InputDomain };
            desc.TargetDomains.AddRange(d.Targets);
            foreach (var (key, thunk) in d.Events)
            {
                desc.Events.Add(CallbackEntry(key, Assign(SagaEventInvoker(thunk))));
            }
            foreach (var (cmd, thunks) in d.Rejections)
            {
                var entry = new Abi.RejectionEntry { FqCommandType = cmd };
                foreach (var thunk in thunks)
                {
                    entry.CallbackIds.Add(Assign(SagaRejectionInvoker(thunk)));
                }
                desc.Rejections.Add(entry);
            }
            Check(Ffi.RegisterSaga(_ptr, desc.ToByteArray()));
        }
    }

    public void RegisterProcessManager<TState>(ProcessManagerDispatch<TState> d)
        where TState : class, IMessage
    {
        lock (_lock)
        {
            var factory = d.Rebuilder.Factory;
            var desc = new Abi.ProcessManagerDescriptor { Name = d.Name, PmDomain = d.PmDomain };
            foreach (var (key, thunk) in d.Rebuilder.Appliers)
            {
                desc.Appliers.Add(CallbackEntry(key, Assign(ApplierInvoker(factory, thunk))));
            }
            if (d.Rebuilder.Snapshot != null)
            {
                desc.SnapshotCallbackId = Assign(ApplierInvoker(factory, d.Rebuilder.Snapshot));
            }
            foreach (var (sourceDomain, byType) in d.Handlers)
            {
                foreach (var (fqType, thunk) in byType)
                {
                    desc.Events.Add(
                        new Abi.PmEventEntry
                        {
                            InputDomain = sourceDomain,
                            FqType = fqType,
                            CallbackId = Assign(PmEventInvoker(factory, thunk)),
                        }
                    );
                }
            }
            foreach (var (cmd, thunks) in d.Rejections)
            {
                var entry = new Abi.RejectionEntry { FqCommandType = cmd };
                foreach (var thunk in thunks)
                {
                    entry.CallbackIds.Add(Assign(PmRejectionInvoker(factory, thunk)));
                }
                desc.Rejections.Add(entry);
            }
            Check(Ffi.RegisterProcessManager(_ptr, desc.ToByteArray()));
        }
    }

    private static Abi.CallbackEntry CallbackEntry(string fqType, ulong id) =>
        new() { FqType = fqType, CallbackId = id };

    private static void Check(int ret)
    {
        if (ret != 0)
        {
            throw Statuses.FromStatusBytes(null, ret);
        }
    }

    // --- dispatch ------------------------------------------------------------

    public BusinessResponse Dispatch(ContextualCommand command) =>
        Parse(DispatchVia(command, Ffi.Dispatch), BusinessResponse.Parser, "BusinessResponse");

    public SagaResponse DispatchSaga(SagaHandleRequest request) =>
        Parse(DispatchVia(request, Ffi.DispatchSaga), SagaResponse.Parser, "SagaResponse");

    public Projection DispatchProjector(EventBook book) =>
        Parse(DispatchVia(book, Ffi.DispatchProjector), Projection.Parser, "Projection");

    public ProcessManagerHandleResponse DispatchProcessManager(
        ProcessManagerHandleRequest request
    ) =>
        Parse(
            DispatchVia(request, Ffi.DispatchProcessManager),
            ProcessManagerHandleResponse.Parser,
            "ProcessManagerHandleResponse"
        );

    private Ffi.Dispatched DispatchVia(
        IMessage request,
        Func<IntPtr, IntPtr, byte[], Ffi.Dispatched> call
    )
    {
        var handle = GCHandle.Alloc(new Session(this));
        try
        {
            return call(_ptr, GCHandle.ToIntPtr(handle), request.ToByteArray());
        }
        finally
        {
            handle.Free();
        }
    }

    private static T Parse<T>(Ffi.Dispatched d, MessageParser<T> parser, string what)
        where T : IMessage<T>
    {
        if (d.Status != 0)
        {
            throw Statuses.FromStatusBytes(d.Response, d.Status);
        }
        try
        {
            return parser.ParseFrom(d.Response ?? Array.Empty<byte>());
        }
        catch (InvalidProtocolBufferException e)
        {
            throw CodedError.Unhandled($"unmarshal {what}: {e.Message}");
        }
    }

    // --- invokers (type-erased bridges; the lone (TState) cast lives here) ----

    private static Any AnyOf(string typeUrl, byte[] payload) =>
        new() { TypeUrl = typeUrl, Value = ByteString.CopyFrom(payload) };

    private static Destinations DestinationsOf(
        Google.Protobuf.Collections.MapField<string, uint> seqs
    ) => new(new Dictionary<string, uint>(seqs));

    private static Invoker ApplierInvoker<TState>(Func<TState> factory, ApplierThunk<TState> thunk)
        where TState : class, IMessage =>
        (session, typeUrl, payload, aux) =>
        {
            thunk((TState)session.EnsureState(factory), AnyOf(typeUrl, payload));
            return new InvokerResult(null, Ffi.StatusOk);
        };

    private static Invoker CommandInvoker<TState>(Func<TState> factory, CommandThunk<TState> thunk)
        where TState : class, IMessage =>
        (session, typeUrl, payload, aux) =>
        {
            var cax = Abi.CommandContextAux.Parser.ParseFrom(aux);
            var cctx = new CommandContext(cax.NextSequence, cax.HadPriorEvents);
            var book = thunk(AnyOf(typeUrl, payload), (TState)session.EnsureState(factory), cctx);
            return book == null
                ? new InvokerResult(null, Ffi.StatusOkEmpty)
                : new InvokerResult(book.ToByteArray(), Ffi.StatusOk);
        };

    private static Invoker RejectionInvoker<TState>(
        Func<TState> factory,
        RejectionThunk<TState> thunk
    )
        where TState : class, IMessage =>
        (session, typeUrl, payload, aux) =>
        {
            var rax = Abi.RejectionAux.Parser.ParseFrom(aux);
            var n = Notification.Parser.ParseFrom(rax.Notification);
            var rej = RejectionNotification.Parser.ParseFrom(rax.Rejection);
            var cctx =
                rax.Cctx != null
                    ? new CommandContext(rax.Cctx.NextSequence, rax.Cctx.HadPriorEvents)
                    : new CommandContext(0, false);
            var resp = thunk(n, rej, (TState)session.EnsureState(factory), cctx);
            return resp == null
                ? new InvokerResult(null, Ffi.StatusOkEmpty)
                : new InvokerResult(resp.ToByteArray(), Ffi.StatusOk);
        };

    private static Invoker ProjectorEventInvoker<TState>(
        Func<TState> factory,
        ProjectorEventThunk<TState> thunk
    )
        where TState : class, IMessage =>
        (session, typeUrl, payload, aux) =>
        {
            thunk((TState)session.EnsureState(factory), AnyOf(typeUrl, payload));
            return new InvokerResult(null, Ffi.StatusOk);
        };

    private static Invoker ProjectorFinishInvoker<TState>(
        Func<TState> factory,
        ProjectorFinishThunk<TState> thunk
    )
        where TState : class, IMessage =>
        (session, typeUrl, payload, aux) =>
        {
            var book = EventBook.Parser.ParseFrom(payload);
            var proj = thunk((TState)session.EnsureState(factory), book);
            return new InvokerResult(proj.ToByteArray(), Ffi.StatusOk);
        };

    private static Invoker ProjectorUnknownInvoker(ProjectorUnknownThunk thunk) =>
        (session, typeUrl, payload, aux) =>
        {
            thunk(typeUrl);
            return new InvokerResult(null, Ffi.StatusOk);
        };

    private static Invoker SagaEventInvoker(SagaEventThunk thunk) =>
        (session, typeUrl, payload, aux) =>
        {
            var sax = Abi.SagaEventAux.Parser.ParseFrom(aux);
            var dests = DestinationsOf(sax.DestinationSequences);
            var emission = thunk(AnyOf(typeUrl, payload), dests, sax.SourceCover);
            var resp = new SagaResponse();
            resp.Commands.AddRange(emission.Commands);
            resp.Events.AddRange(emission.Events);
            return new InvokerResult(resp.ToByteArray(), Ffi.StatusOk);
        };

    private static Invoker SagaRejectionInvoker(SagaRejectionThunk thunk) =>
        (session, typeUrl, payload, aux) =>
        {
            var rax = Abi.RejectionAux.Parser.ParseFrom(aux);
            var n = Notification.Parser.ParseFrom(rax.Notification);
            var rej = RejectionNotification.Parser.ParseFrom(rax.Rejection);
            var resp = new SagaResponse();
            resp.Events.AddRange(thunk(n, rej));
            return new InvokerResult(resp.ToByteArray(), Ffi.StatusOk);
        };

    private static Invoker PmEventInvoker<TState>(Func<TState> factory, PmEventThunk<TState> thunk)
        where TState : class, IMessage =>
        (session, typeUrl, payload, aux) =>
        {
            var pax = Abi.PmEventAux.Parser.ParseFrom(aux);
            var dests = DestinationsOf(pax.DestinationSequences);
            var resp = thunk(AnyOf(typeUrl, payload), (TState)session.EnsureState(factory), dests);
            return new InvokerResult(resp.ToByteArray(), Ffi.StatusOk);
        };

    private static Invoker PmRejectionInvoker<TState>(
        Func<TState> factory,
        PmRejectionThunk<TState> thunk
    )
        where TState : class, IMessage =>
        (session, typeUrl, payload, aux) =>
        {
            var rax = Abi.RejectionAux.Parser.ParseFrom(aux);
            var n = Notification.Parser.ParseFrom(rax.Notification);
            var rej = RejectionNotification.Parser.ParseFrom(rax.Rejection);
            var r = thunk(n, rej, (TState)session.EnsureState(factory));
            var resp = new ProcessManagerHandleResponse();
            resp.ProcessEvents.AddRange(r.ProcessEvents);
            if (r.Escalation != null)
            {
                resp.Notification = r.Escalation;
            }
            return new InvokerResult(resp.ToByteArray(), Ffi.StatusOk);
        };
}
