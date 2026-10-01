package io.angzarr.router;

import com.google.protobuf.Any;
import com.google.protobuf.ByteString;
import com.google.protobuf.InvalidProtocolBufferException;
import com.google.protobuf.Message;
import io.angzarr.BusinessResponse;
import io.angzarr.Compensate;
import io.angzarr.ContextualCommand;
import io.angzarr.EventBook;
import io.angzarr.FactRequest;
import io.angzarr.Notification;
import io.angzarr.ProcessManagerHandleRequest;
import io.angzarr.ProcessManagerHandleResponse;
import io.angzarr.Projection;
import io.angzarr.RejectionNotification;
import io.angzarr.ReplayRequest;
import io.angzarr.ReplayResponse;
import io.angzarr.SagaHandleRequest;
import io.angzarr.SagaResponse;
import io.angzarr.router.Thunks.ApplierThunk;
import io.angzarr.router.Thunks.CommandThunk;
import io.angzarr.router.Thunks.FactThunk;
import io.angzarr.router.Thunks.PmCompensatorThunk;
import io.angzarr.router.Thunks.PmEventCoverThunk;
import io.angzarr.router.Thunks.ProjectorEventContextThunk;
import io.angzarr.router.Thunks.ProjectorFinishThunk;
import io.angzarr.router.Thunks.ProjectorUnknownThunk;
import io.angzarr.router.Thunks.RejectionThunk;
import io.angzarr.router.Thunks.SagaEmission;
import io.angzarr.router.Thunks.SagaEventThunk;
import io.angzarr.router.Thunks.UndoThunk;
import io.angzarr.router.ffi.v1.Abi;
import java.lang.foreign.MemorySegment;
import java.util.List;
import java.util.Map;
import java.util.concurrent.ConcurrentHashMap;
import java.util.concurrent.atomic.AtomicLong;
import java.util.concurrent.locks.ReentrantReadWriteLock;
import java.util.function.Function;
import java.util.function.Supplier;

/**
 * The Java binding's router: wraps the native router plus the host-side callback registry the core
 * reaches through the single callback gateway. Register a component (assigning callback ids to its
 * thunks and handing the core a serialized descriptor), then dispatch books/commands through it.
 */
public final class Router implements AutoCloseable {

  // The native router; null once closed. Dispatch and registration hold the
  // read lock for the duration of their native call, close takes the write
  // lock, so the router is freed exactly once and never while in use.
  private MemorySegment ptr;
  private final ReentrantReadWriteLock lifecycle = new ReentrantReadWriteLock();
  private final ConcurrentHashMap<Long, Invoker> registry = new ConcurrentHashMap<>();
  private final AtomicLong nextId = new AtomicLong(0);
  // Each registered component's key into the per-dispatch Session state map.
  private final AtomicLong nextComponent = new AtomicLong(0);

  public Router() {
    this.ptr = Ffi.routerNew();
  }

  /** The ABI version the loaded router-ffi library reports. */
  public static int abiVersion() {
    return Ffi.abiVersion();
  }

  /**
   * Releases the native router. Idempotent: only the first call frees it; afterwards dispatch and
   * registration throw {@link IllegalStateException}.
   */
  @Override
  public void close() {
    lifecycle.writeLock().lock();
    try {
      if (ptr != null) {
        MemorySegment p = ptr;
        ptr = null;
        Ffi.routerFree(p);
      }
    } finally {
      lifecycle.writeLock().unlock();
    }
  }

  /** Runs one native call against the live router, refusing a closed one. */
  private <T> T withRouter(Function<MemorySegment, T> call) {
    lifecycle.readLock().lock();
    try {
      if (ptr == null) {
        throw new IllegalStateException("angzarr router is closed");
      }
      return call.apply(ptr);
    } finally {
      lifecycle.readLock().unlock();
    }
  }

  Invoker invokerFor(long callbackId) {
    return registry.get(callbackId);
  }

  private long assign(Invoker invoker) {
    long id = nextId.incrementAndGet();
    registry.put(id, invoker);
    return id;
  }

  // --- registration --------------------------------------------------------

  public synchronized void registerAggregate(AggregateDispatch d) {
    Supplier<Message.Builder> factory = d.rebuilder.factory;
    long component = nextComponent.incrementAndGet();
    Abi.AggregateDescriptor.Builder desc =
        Abi.AggregateDescriptor.newBuilder().setName(d.name).setDomain(d.domain);

    for (Map.Entry<String, ApplierThunk> e : d.rebuilder.appliers.entrySet()) {
      long id = assign(applierInvoker(component, factory, e.getValue()));
      desc.addAppliers(callbackEntry(e.getKey(), id));
    }
    if (d.rebuilder.snapshot != null) {
      desc.setSnapshotCallbackId(assign(applierInvoker(component, factory, d.rebuilder.snapshot)));
    }
    for (Map.Entry<String, CommandThunk> e : d.commands.entrySet()) {
      long id = assign(commandInvoker(component, factory, e.getValue()));
      desc.addCommands(callbackEntry(e.getKey(), id));
    }
    for (Map.Entry<String, List<RejectionThunk>> e : d.rejections.entrySet()) {
      Abi.RejectionEntry.Builder entry = Abi.RejectionEntry.newBuilder().setCompensates(e.getKey());
      for (RejectionThunk thunk : e.getValue()) {
        entry.addCallbackIds(assign(rejectionInvoker(component, factory, thunk)));
      }
      desc.addRejections(entry);
    }
    for (Map.Entry<String, UndoThunk> e : d.undoes.entrySet()) {
      long id = assign(undoInvoker(component, factory, e.getValue()));
      desc.addUndoes(callbackEntry(e.getKey(), id));
    }
    for (Map.Entry<String, FactThunk> e : d.facts.entrySet()) {
      long id = assign(factInvoker(component, factory, e.getValue()));
      desc.addFacts(callbackEntry(e.getKey(), id));
    }
    desc.setStateCallbackId(assign(stateInvoker(component, factory)));
    byte[] descriptor = desc.build().toByteArray();
    check(withRouter(p -> Ffi.registerAggregate(p, descriptor)));
  }

  public synchronized void registerProjector(ProjectorDispatch d) {
    Supplier<Message.Builder> factory = d.factory;
    long component = nextComponent.incrementAndGet();
    Abi.ProjectorDescriptor.Builder desc =
        Abi.ProjectorDescriptor.newBuilder().setName(d.name).addAllDomains(d.domains);

    for (Map.Entry<String, ProjectorEventContextThunk> e : d.events.entrySet()) {
      long id = assign(projectorEventInvoker(component, factory, e.getValue()));
      desc.addEvents(callbackEntry(e.getKey(), id));
    }
    if (d.unknown != null) {
      desc.setUnknownCallbackId(assign(projectorUnknownInvoker(d.unknown)));
    }
    if (d.finish != null) {
      desc.setFinishCallbackId(assign(projectorFinishInvoker(component, factory, d.finish)));
    }
    byte[] descriptor = desc.build().toByteArray();
    check(withRouter(p -> Ffi.registerProjector(p, descriptor)));
  }

  public synchronized void registerSaga(SagaDispatch d) {
    Destinations dests = new Destinations(d.targets);
    Abi.SagaDescriptor.Builder desc =
        Abi.SagaDescriptor.newBuilder()
            .setName(d.name)
            .setInputDomain(d.inputDomain)
            .addAllTargetDomains(d.targets);

    for (Map.Entry<String, SagaEventThunk> e : d.events.entrySet()) {
      long id = assign(sagaEventInvoker(dests, e.getValue()));
      desc.addEvents(callbackEntry(e.getKey(), id));
    }
    byte[] descriptor = desc.build().toByteArray();
    check(withRouter(p -> Ffi.registerSaga(p, descriptor)));
  }

  public synchronized void registerProcessManager(ProcessManagerDispatch d) {
    Supplier<Message.Builder> factory = d.rebuilder.factory;
    long component = nextComponent.incrementAndGet();
    Abi.ProcessManagerDescriptor.Builder desc =
        Abi.ProcessManagerDescriptor.newBuilder()
            .setName(d.name)
            .setPmDomain(d.pmDomain)
            .addAllTargetDomains(d.targets);
    Destinations dests = new Destinations(d.targets);

    for (Map.Entry<String, ApplierThunk> e : d.rebuilder.appliers.entrySet()) {
      long id = assign(applierInvoker(component, factory, e.getValue()));
      desc.addAppliers(callbackEntry(e.getKey(), id));
    }
    if (d.rebuilder.snapshot != null) {
      desc.setSnapshotCallbackId(assign(applierInvoker(component, factory, d.rebuilder.snapshot)));
    }
    for (Map.Entry<String, Map<String, PmEventCoverThunk>> byDomain : d.handlers.entrySet()) {
      for (Map.Entry<String, PmEventCoverThunk> e : byDomain.getValue().entrySet()) {
        long id = assign(pmEventInvoker(component, factory, dests, e.getValue()));
        desc.addEvents(
            Abi.PmEventEntry.newBuilder()
                .setInputDomain(byDomain.getKey())
                .setFqType(e.getKey())
                .setCallbackId(id));
      }
    }
    for (Map.Entry<String, List<PmCompensatorThunk>> e : d.rejections.entrySet()) {
      Abi.RejectionEntry.Builder entry = Abi.RejectionEntry.newBuilder().setCompensates(e.getKey());
      for (PmCompensatorThunk thunk : e.getValue()) {
        entry.addCallbackIds(assign(pmRejectionInvoker(component, factory, thunk)));
      }
      desc.addRejections(entry);
    }
    byte[] descriptor = desc.build().toByteArray();
    check(withRouter(p -> Ffi.registerProcessManager(p, descriptor)));
  }

  private static Abi.CallbackEntry.Builder callbackEntry(String fqType, long id) {
    return Abi.CallbackEntry.newBuilder().setFqType(fqType).setCallbackId(id);
  }

  private static void check(int ret) {
    if (ret != 0) {
      throw Statuses.fromStatusBytes(null, ret);
    }
  }

  // --- dispatch ------------------------------------------------------------

  public BusinessResponse dispatch(ContextualCommand command) {
    Ffi.Dispatched d = dispatch(command, Ffi::dispatch);
    return parse(d, BusinessResponse::parseFrom, "BusinessResponse");
  }

  public SagaResponse dispatchSaga(SagaHandleRequest request) {
    Ffi.Dispatched d = dispatch(request, Ffi::dispatchSaga);
    return parse(d, SagaResponse::parseFrom, "SagaResponse");
  }

  public Projection dispatchProjector(EventBook book) {
    Ffi.Dispatched d = dispatch(book, Ffi::dispatchProjector);
    return parse(d, Projection::parseFrom, "Projection");
  }

  public ProcessManagerHandleResponse dispatchProcessManager(ProcessManagerHandleRequest request) {
    Ffi.Dispatched d = dispatch(request, Ffi::dispatchProcessManager);
    return parse(d, ProcessManagerHandleResponse::parseFrom, "ProcessManagerHandleResponse");
  }

  /**
   * Runs facts through the fact handling of the aggregate claiming the facts' cover domain (a sole
   * aggregate claims everything); returns the facts to record.
   */
  public EventBook dispatchFact(FactRequest request) {
    Ffi.Dispatched d = dispatch(request, Ffi::dispatchFact);
    return parse(d, EventBook::parseFrom, "EventBook");
  }

  /**
   * Replays a snapshot and events through the appliers of the aggregate registered for domain (an
   * empty domain selects a sole registered aggregate); returns its packed state.
   */
  public ReplayResponse dispatchReplay(String domain, ReplayRequest request) {
    Abi.ReplayCall call =
        Abi.ReplayCall.newBuilder()
            .setDomain(domain == null ? "" : domain)
            .setRequest(request)
            .build();
    Ffi.Dispatched d = dispatch(call, Ffi::dispatchReplay);
    return parse(d, ReplayResponse::parseFrom, "ReplayResponse");
  }

  @FunctionalInterface
  private interface DispatchCall {
    Ffi.Dispatched call(MemorySegment router, long sessionId, byte[] request);
  }

  @FunctionalInterface
  private interface ProtoParser<T> {
    T parse(byte[] bytes) throws InvalidProtocolBufferException;
  }

  private Ffi.Dispatched dispatch(Message request, DispatchCall call) {
    long sessionId = Ffi.openSession(new Session(this));
    try {
      byte[] bytes = request.toByteArray();
      return withRouter(p -> call.call(p, sessionId, bytes));
    } finally {
      Ffi.closeSession(sessionId);
    }
  }

  private static <T> T parse(Ffi.Dispatched d, ProtoParser<T> parser, String what) {
    if (d.status() != 0) {
      throw Statuses.fromStatusBytes(d.response(), d.status());
    }
    try {
      return parser.parse(d.response());
    } catch (InvalidProtocolBufferException e) {
      throw CodedError.unhandled("unmarshal " + what + ": " + e.getMessage());
    }
  }

  // --- invokers (type-erased bridges, mirroring the Go binding) ------------

  private static Any anyOf(String typeUrl, byte[] payload) {
    return Any.newBuilder().setTypeUrl(typeUrl).setValue(ByteString.copyFrom(payload)).build();
  }

  private static CommandContext commandContext(Abi.CommandContextAux cax) {
    return new CommandContext(
        Integer.toUnsignedLong(cax.getNextSequence()), cax.getHadPriorEvents(), cax.getCover());
  }

  private static Invoker applierInvoker(
      long component, Supplier<Message.Builder> factory, ApplierThunk thunk) {
    return (session, typeUrl, payload, aux) -> {
      thunk.apply(session.ensureState(component, factory), anyOf(typeUrl, payload));
      return new Invoker.Result(null, Ffi.STATUS_OK);
    };
  }

  private static Invoker commandInvoker(
      long component, Supplier<Message.Builder> factory, CommandThunk thunk) {
    return (session, typeUrl, payload, aux) -> {
      CommandContext cctx = commandContext(Abi.CommandContextAux.parseFrom(aux));
      EventBook book =
          thunk.handle(anyOf(typeUrl, payload), session.ensureState(component, factory), cctx);
      if (book == null) {
        return new Invoker.Result(null, Ffi.STATUS_OK_EMPTY);
      }
      return new Invoker.Result(book.toByteArray(), Ffi.STATUS_OK);
    };
  }

  private static Invoker rejectionInvoker(
      long component, Supplier<Message.Builder> factory, RejectionThunk thunk) {
    return (session, typeUrl, payload, aux) -> {
      Abi.RejectionAux rax = Abi.RejectionAux.parseFrom(aux);
      Notification n = Notification.parseFrom(rax.getNotification());
      RejectionNotification rej = RejectionNotification.parseFrom(rax.getRejection());
      CommandContext cctx = commandContext(rax.getCctx());
      BusinessResponse resp =
          thunk.compensate(n, rej, session.ensureState(component, factory), cctx);
      if (resp == null) {
        return new Invoker.Result(null, Ffi.STATUS_OK_EMPTY);
      }
      return new Invoker.Result(resp.toByteArray(), Ffi.STATUS_OK);
    };
  }

  private static Invoker undoInvoker(
      long component, Supplier<Message.Builder> factory, UndoThunk thunk) {
    return (session, typeUrl, payload, aux) -> {
      Abi.UndoAux uax = Abi.UndoAux.parseFrom(aux);
      Notification n = Notification.parseFrom(uax.getNotification());
      Compensate compensate = Compensate.parseFrom(uax.getCompensate());
      BusinessResponse resp =
          thunk.undo(
              n,
              compensate,
              session.ensureState(component, factory),
              commandContext(uax.getCctx()));
      if (resp == null) {
        return new Invoker.Result(null, Ffi.STATUS_OK_EMPTY);
      }
      return new Invoker.Result(resp.toByteArray(), Ffi.STATUS_OK);
    };
  }

  private static Invoker factInvoker(
      long component, Supplier<Message.Builder> factory, FactThunk thunk) {
    return (session, typeUrl, payload, aux) -> {
      Any recorded = thunk.handle(anyOf(typeUrl, payload), session.ensureState(component, factory));
      if (recorded == null) {
        return new Invoker.Result(null, Ffi.STATUS_OK_EMPTY);
      }
      return new Invoker.Result(recorded.toByteArray(), Ffi.STATUS_OK);
    };
  }

  /** Packs the component's current state (google.protobuf.Any, bare "/" prefix) for Replay. */
  private static Invoker stateInvoker(long component, Supplier<Message.Builder> factory) {
    return (session, typeUrl, payload, aux) -> {
      Any state = Pack.pack(session.ensureState(component, factory).build());
      return new Invoker.Result(state.toByteArray(), Ffi.STATUS_OK);
    };
  }

  private static Invoker projectorEventInvoker(
      long component, Supplier<Message.Builder> factory, ProjectorEventContextThunk thunk) {
    return (session, typeUrl, payload, aux) -> {
      Abi.ProjectorEventAux pax = Abi.ProjectorEventAux.parseFrom(aux);
      PageContext ctx = new PageContext(pax.getCover(), Integer.toUnsignedLong(pax.getSequence()));
      thunk.fold(session.ensureState(component, factory), anyOf(typeUrl, payload), ctx);
      return new Invoker.Result(null, Ffi.STATUS_OK);
    };
  }

  private static Invoker projectorFinishInvoker(
      long component, Supplier<Message.Builder> factory, ProjectorFinishThunk thunk) {
    return (session, typeUrl, payload, aux) -> {
      EventBook book = EventBook.parseFrom(payload);
      Projection proj = thunk.finish(session.ensureState(component, factory), book);
      return new Invoker.Result(proj.toByteArray(), Ffi.STATUS_OK);
    };
  }

  private static Invoker projectorUnknownInvoker(ProjectorUnknownThunk thunk) {
    return (session, typeUrl, payload, aux) -> {
      thunk.onUnknown(typeUrl);
      return new Invoker.Result(null, Ffi.STATUS_OK);
    };
  }

  private static Invoker sagaEventInvoker(Destinations dests, SagaEventThunk thunk) {
    return (session, typeUrl, payload, aux) -> {
      Abi.SagaEventAux sax = Abi.SagaEventAux.parseFrom(aux);
      SagaEmission emission = thunk.translate(anyOf(typeUrl, payload), dests, sax.getSourceCover());
      SagaResponse resp =
          SagaResponse.newBuilder()
              .addAllCommands(emission.commands())
              .addAllEvents(emission.events())
              .build();
      return new Invoker.Result(resp.toByteArray(), Ffi.STATUS_OK);
    };
  }

  private static Invoker pmEventInvoker(
      long component,
      Supplier<Message.Builder> factory,
      Destinations dests,
      PmEventCoverThunk thunk) {
    return (session, typeUrl, payload, aux) -> {
      Abi.PmEventAux pax = Abi.PmEventAux.parseFrom(aux);
      ProcessManagerHandleResponse resp =
          thunk.handle(
              anyOf(typeUrl, payload),
              session.ensureState(component, factory),
              dests,
              pax.getTriggerCover());
      if (resp == null) {
        return new Invoker.Result(null, Ffi.STATUS_OK_EMPTY);
      }
      return new Invoker.Result(resp.toByteArray(), Ffi.STATUS_OK);
    };
  }

  private static Invoker pmRejectionInvoker(
      long component, Supplier<Message.Builder> factory, PmCompensatorThunk thunk) {
    return (session, typeUrl, payload, aux) -> {
      Abi.RejectionAux rax = Abi.RejectionAux.parseFrom(aux);
      Notification n = Notification.parseFrom(rax.getNotification());
      RejectionNotification rej = RejectionNotification.parseFrom(rax.getRejection());
      ProcessManagerHandleResponse resp =
          thunk.compensate(n, rej, session.ensureState(component, factory));
      if (resp == null) {
        return new Invoker.Result(null, Ffi.STATUS_OK_EMPTY);
      }
      return new Invoker.Result(resp.toByteArray(), Ffi.STATUS_OK);
    };
  }
}
