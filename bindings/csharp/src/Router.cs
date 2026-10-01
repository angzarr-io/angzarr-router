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
/// (<c>(TState)session.EnsureState(component, ...)</c>). It is guaranteed correct
/// because each registration mints its own <see cref="ComponentKey"/> that only
/// that component's invokers capture, and the session creates the state for a
/// key from that component's own factory.</para>
/// </summary>
public sealed class Router : IDisposable
{
    private readonly RouterHandle _handle;
    private readonly ConcurrentDictionary<ulong, Invoker> _registry = new();
    private readonly object _lock = new();
    private long _nextId;

    public Router()
    {
        _handle = Ffi.RouterNew();
        if (_handle.IsInvalid)
        {
            throw new InvalidOperationException("angzarr_router_new returned null");
        }
    }

    /// <summary>The router-ffi ABI version this binding requires; a loaded
    /// library reporting any other version is refused at load.</summary>
    public const int ExpectedAbiVersion = (int)Ffi.ExpectedAbiVersion;

    /// <summary>The ABI version the loaded router-ffi library reports.</summary>
    public static int AbiVersion() => (int)Ffi.AbiVersion();

    /// <summary>True once <see cref="Dispose"/> has released the native
    /// router.</summary>
    public bool IsDisposed => _handle.IsClosed;

    /// <summary>Releases the native router. Idempotent; registering or
    /// dispatching afterwards throws <see cref="ObjectDisposedException"/>. A
    /// router never disposed is released by its handle's finalizer.</summary>
    public void Dispose() => _handle.Dispose();

    private RouterHandle Handle()
    {
        if (_handle.IsClosed)
        {
            throw new ObjectDisposedException(nameof(Router));
        }
        return _handle;
    }

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
            var component = new ComponentKey();
            var factory = d.Rebuilder.Factory;
            var desc = new Abi.AggregateDescriptor { Name = d.Name, Domain = d.Domain };
            foreach (var (key, thunk) in d.Rebuilder.Appliers)
            {
                desc.Appliers.Add(
                    CallbackEntry(key, Assign(ApplierInvoker(component, factory, thunk)))
                );
            }
            if (d.Rebuilder.Snapshot != null)
            {
                desc.SnapshotCallbackId = Assign(
                    SnapshotInvoker(component, factory, d.Rebuilder.Snapshot)
                );
            }
            foreach (var (key, thunk) in d.Commands)
            {
                desc.Commands.Add(
                    CallbackEntry(key, Assign(CommandInvoker(component, factory, thunk)))
                );
            }
            foreach (var (compensates, thunks) in d.Rejections)
            {
                var entry = new Abi.RejectionEntry { Compensates = compensates };
                foreach (var thunk in thunks)
                {
                    entry.CallbackIds.Add(Assign(RejectionInvoker(component, factory, thunk)));
                }
                desc.Rejections.Add(entry);
            }
            foreach (var (key, thunk) in d.Undoes)
            {
                desc.Undoes.Add(CallbackEntry(key, Assign(UndoInvoker(component, factory, thunk))));
            }
            foreach (var (key, thunk) in d.Facts)
            {
                desc.Facts.Add(CallbackEntry(key, Assign(FactInvoker(component, factory, thunk))));
            }
            desc.StateCallbackId = Assign(StateInvoker(component, factory));
            Check(Ffi.RegisterAggregate(Handle(), desc.ToByteArray()));
        }
    }

    public void RegisterProjector<TState>(ProjectorDispatch<TState> d)
        where TState : class, IMessage
    {
        lock (_lock)
        {
            var component = new ComponentKey();
            var factory = d.Factory;
            var desc = new Abi.ProjectorDescriptor { Name = d.Name };
            desc.Domains.AddRange(d.Domains);
            foreach (var (key, thunk) in d.Events)
            {
                desc.Events.Add(
                    CallbackEntry(key, Assign(ProjectorEventInvoker(component, factory, thunk)))
                );
            }
            if (d.Unknown != null)
            {
                desc.UnknownCallbackId = Assign(ProjectorUnknownInvoker(d.Unknown));
            }
            if (d.FinishThunk != null)
            {
                desc.FinishCallbackId = Assign(
                    ProjectorFinishInvoker(component, factory, d.FinishThunk)
                );
            }
            Check(Ffi.RegisterProjector(Handle(), desc.ToByteArray()));
        }
    }

    public void RegisterSaga(SagaDispatch d)
    {
        lock (_lock)
        {
            var desc = new Abi.SagaDescriptor { Name = d.Name, InputDomain = d.InputDomain };
            desc.TargetDomains.AddRange(d.Targets);
            var dests = new Destinations(d.Targets);
            foreach (var (key, thunk) in d.Events)
            {
                desc.Events.Add(CallbackEntry(key, Assign(SagaEventInvoker(dests, thunk))));
            }
            Check(Ffi.RegisterSaga(Handle(), desc.ToByteArray()));
        }
    }

    public void RegisterProcessManager<TState>(ProcessManagerDispatch<TState> d)
        where TState : class, IMessage
    {
        lock (_lock)
        {
            var component = new ComponentKey();
            var factory = d.Rebuilder.Factory;
            var desc = new Abi.ProcessManagerDescriptor { Name = d.Name, PmDomain = d.PmDomain };
            desc.TargetDomains.AddRange(d.Targets);
            var dests = new Destinations(d.Targets);
            foreach (var (key, thunk) in d.Rebuilder.Appliers)
            {
                desc.Appliers.Add(
                    CallbackEntry(key, Assign(ApplierInvoker(component, factory, thunk)))
                );
            }
            if (d.Rebuilder.Snapshot != null)
            {
                desc.SnapshotCallbackId = Assign(
                    SnapshotInvoker(component, factory, d.Rebuilder.Snapshot)
                );
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
                            CallbackId = Assign(PmEventInvoker(component, factory, dests, thunk)),
                        }
                    );
                }
            }
            foreach (var (compensates, thunks) in d.Rejections)
            {
                var entry = new Abi.RejectionEntry { Compensates = compensates };
                foreach (var thunk in thunks)
                {
                    entry.CallbackIds.Add(Assign(PmRejectionInvoker(component, factory, thunk)));
                }
                desc.Rejections.Add(entry);
            }
            desc.StateCallbackId = Assign(StateInvoker(component, factory));
            Check(Ffi.RegisterProcessManager(Handle(), desc.ToByteArray()));
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

    /// <summary>Handles a FactRequest through the aggregate claiming the facts'
    /// cover domain (a sole aggregate claims everything): each fact folds
    /// against the state rebuilt from the prior events and its fact handler may
    /// annotate it. Returns the EventBook of facts to record.</summary>
    public EventBook DispatchFact(FactRequest request) =>
        Parse(DispatchVia(request, Ffi.DispatchFact), EventBook.Parser, "EventBook");

    /// <summary>Replays a ReplayRequest (base snapshot, then events) through
    /// the aggregate registered for <paramref name="domain"/>, else the process
    /// manager whose own domain it is (empty selects a sole registered
    /// aggregate), and returns its state packed as an Any.</summary>
    public ReplayResponse DispatchReplay(string domain, ReplayRequest request) =>
        Parse(
            DispatchVia(
                new Abi.ReplayCall { Domain = domain, Request = request },
                Ffi.DispatchReplay
            ),
            ReplayResponse.Parser,
            "ReplayResponse"
        );

    private Ffi.Dispatched DispatchVia(
        IMessage request,
        Func<RouterHandle, IntPtr, byte[], Ffi.Dispatched> call
    )
    {
        var router = Handle();
        var handle = GCHandle.Alloc(new Session(this));
        try
        {
            return call(router, GCHandle.ToIntPtr(handle), request.ToByteArray());
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

    private static CommandContext ContextOf(Abi.CommandContextAux? cax) =>
        cax == null
            ? new CommandContext(0, false)
            : new CommandContext(cax.NextSequence, cax.HadPriorEvents, cax.Cover);

    private static PageContext PageOf(byte[] aux)
    {
        var pax = Abi.ProjectorEventAux.Parser.ParseFrom(aux);
        return new PageContext(pax.Cover, pax.Sequence);
    }

    private static Invoker ApplierInvoker<TState>(
        ComponentKey component,
        Func<TState> factory,
        ApplierPageThunk<TState> thunk
    )
        where TState : class, IMessage =>
        (session, typeUrl, payload, aux) =>
        {
            thunk(
                (TState)session.EnsureState(component, factory),
                AnyOf(typeUrl, payload),
                PageOf(aux)
            );
            return new InvokerResult(null, Ffi.StatusOk);
        };

    private static Invoker SnapshotInvoker<TState>(
        ComponentKey component,
        Func<TState> factory,
        ApplierThunk<TState> thunk
    )
        where TState : class, IMessage =>
        (session, typeUrl, payload, aux) =>
        {
            thunk((TState)session.EnsureState(component, factory), AnyOf(typeUrl, payload));
            return new InvokerResult(null, Ffi.StatusOk);
        };

    private static Invoker CommandInvoker<TState>(
        ComponentKey component,
        Func<TState> factory,
        CommandThunk<TState> thunk
    )
        where TState : class, IMessage =>
        (session, typeUrl, payload, aux) =>
        {
            var cctx = ContextOf(Abi.CommandContextAux.Parser.ParseFrom(aux));
            var book = thunk(
                AnyOf(typeUrl, payload),
                (TState)session.EnsureState(component, factory),
                cctx
            );
            return book == null
                ? new InvokerResult(null, Ffi.StatusOkEmpty)
                : new InvokerResult(book.ToByteArray(), Ffi.StatusOk);
        };

    private static Invoker RejectionInvoker<TState>(
        ComponentKey component,
        Func<TState> factory,
        RejectionThunk<TState> thunk
    )
        where TState : class, IMessage =>
        (session, typeUrl, payload, aux) =>
        {
            var rax = Abi.RejectionAux.Parser.ParseFrom(aux);
            var n = Notification.Parser.ParseFrom(rax.Notification);
            var rej = RejectionNotification.Parser.ParseFrom(rax.Rejection);
            var resp = thunk(
                n,
                rej,
                (TState)session.EnsureState(component, factory),
                ContextOf(rax.Cctx)
            );
            return resp == null
                ? new InvokerResult(null, Ffi.StatusOkEmpty)
                : new InvokerResult(resp.ToByteArray(), Ffi.StatusOk);
        };

    private static Invoker UndoInvoker<TState>(
        ComponentKey component,
        Func<TState> factory,
        UndoThunk<TState> thunk
    )
        where TState : class, IMessage =>
        (session, typeUrl, payload, aux) =>
        {
            var uax = Abi.UndoAux.Parser.ParseFrom(aux);
            var resp = thunk(
                Notification.Parser.ParseFrom(uax.Notification),
                Compensate.Parser.ParseFrom(uax.Compensate),
                (TState)session.EnsureState(component, factory),
                ContextOf(uax.Cctx)
            );
            return resp == null
                ? new InvokerResult(null, Ffi.StatusOkEmpty)
                : new InvokerResult(resp.ToByteArray(), Ffi.StatusOk);
        };

    private static Invoker FactInvoker<TState>(
        ComponentKey component,
        Func<TState> factory,
        FactThunk<TState> thunk
    )
        where TState : class, IMessage =>
        (session, typeUrl, payload, aux) =>
        {
            var recorded = thunk(
                AnyOf(typeUrl, payload),
                (TState)session.EnsureState(component, factory)
            );
            return recorded == null
                ? new InvokerResult(null, Ffi.StatusOkEmpty)
                : new InvokerResult(recorded.ToByteArray(), Ffi.StatusOk);
        };

    private static Invoker StateInvoker<TState>(ComponentKey component, Func<TState> factory)
        where TState : class, IMessage =>
        (session, typeUrl, payload, aux) =>
            new InvokerResult(
                Pack.Wrap(session.EnsureState(component, factory)).ToByteArray(),
                Ffi.StatusOk
            );

    private static Invoker ProjectorEventInvoker<TState>(
        ComponentKey component,
        Func<TState> factory,
        ProjectorPageThunk<TState> thunk
    )
        where TState : class, IMessage =>
        (session, typeUrl, payload, aux) =>
        {
            thunk(
                (TState)session.EnsureState(component, factory),
                AnyOf(typeUrl, payload),
                PageOf(aux)
            );
            return new InvokerResult(null, Ffi.StatusOk);
        };

    private static Invoker ProjectorFinishInvoker<TState>(
        ComponentKey component,
        Func<TState> factory,
        ProjectorFinishThunk<TState> thunk
    )
        where TState : class, IMessage =>
        (session, typeUrl, payload, aux) =>
        {
            var book = EventBook.Parser.ParseFrom(payload);
            var proj = thunk((TState)session.EnsureState(component, factory), book);
            return new InvokerResult(proj.ToByteArray(), Ffi.StatusOk);
        };

    private static Invoker ProjectorUnknownInvoker(ProjectorUnknownThunk thunk) =>
        (session, typeUrl, payload, aux) =>
        {
            thunk(typeUrl);
            return new InvokerResult(null, Ffi.StatusOk);
        };

    private static Invoker SagaEventInvoker(Destinations dests, SagaEventPageThunk thunk) =>
        (session, typeUrl, payload, aux) =>
        {
            var sax = Abi.SagaEventAux.Parser.ParseFrom(aux);
            var source = new PageContext(sax.SourceCover, sax.SourceSeq);
            var emission = thunk(AnyOf(typeUrl, payload), dests, source);
            var resp = new SagaResponse();
            resp.Commands.AddRange(emission.Commands);
            resp.Events.AddRange(emission.Events);
            return new InvokerResult(resp.ToByteArray(), Ffi.StatusOk);
        };

    private static Invoker PmEventInvoker<TState>(
        ComponentKey component,
        Func<TState> factory,
        Destinations dests,
        PmTriggerThunk<TState> thunk
    )
        where TState : class, IMessage =>
        (session, typeUrl, payload, aux) =>
        {
            var pax = Abi.PmEventAux.Parser.ParseFrom(aux);
            var resp = thunk(
                AnyOf(typeUrl, payload),
                (TState)session.EnsureState(component, factory),
                dests,
                pax.TriggerCover
            );
            return new InvokerResult(resp.ToByteArray(), Ffi.StatusOk);
        };

    private static Invoker PmRejectionInvoker<TState>(
        ComponentKey component,
        Func<TState> factory,
        PmCompensatorThunk<TState> thunk
    )
        where TState : class, IMessage =>
        (session, typeUrl, payload, aux) =>
        {
            var rax = Abi.RejectionAux.Parser.ParseFrom(aux);
            var n = Notification.Parser.ParseFrom(rax.Notification);
            var rej = RejectionNotification.Parser.ParseFrom(rax.Rejection);
            var resp = thunk(n, rej, (TState)session.EnsureState(component, factory));
            return new InvokerResult(resp.ToByteArray(), Ffi.StatusOk);
        };
}
