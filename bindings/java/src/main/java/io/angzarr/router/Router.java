package io.angzarr.router;

import com.google.protobuf.Any;
import com.google.protobuf.ByteString;
import com.google.protobuf.InvalidProtocolBufferException;
import com.google.protobuf.Message;
import io.angzarr.BusinessResponse;
import io.angzarr.ContextualCommand;
import io.angzarr.EventBook;
import io.angzarr.Notification;
import io.angzarr.ProcessManagerHandleRequest;
import io.angzarr.ProcessManagerHandleResponse;
import io.angzarr.Projection;
import io.angzarr.RejectionNotification;
import io.angzarr.SagaHandleRequest;
import io.angzarr.SagaResponse;
import io.angzarr.router.Thunks.ApplierThunk;
import io.angzarr.router.Thunks.CommandThunk;
import io.angzarr.router.Thunks.PmEventThunk;
import io.angzarr.router.Thunks.PmRejectionThunk;
import io.angzarr.router.Thunks.ProjectorEventThunk;
import io.angzarr.router.Thunks.ProjectorFinishThunk;
import io.angzarr.router.Thunks.ProjectorUnknownThunk;
import io.angzarr.router.Thunks.RejectionThunk;
import io.angzarr.router.Thunks.SagaEmission;
import io.angzarr.router.Thunks.SagaEventThunk;
import io.angzarr.router.Thunks.SagaRejectionThunk;
import io.angzarr.router.ffi.v1.Abi;
import java.lang.foreign.MemorySegment;
import java.util.List;
import java.util.Map;
import java.util.concurrent.ConcurrentHashMap;
import java.util.concurrent.atomic.AtomicLong;
import java.util.function.Supplier;

/**
 * The Java binding's router: wraps the native router plus the host-side callback
 * registry the core reaches through the single callback gateway. Register a
 * component (assigning callback ids to its thunks and handing the core a
 * serialized descriptor), then dispatch books/commands through it.
 */
public final class Router implements AutoCloseable {

  private final MemorySegment ptr;
  private final ConcurrentHashMap<Long, Invoker> registry = new ConcurrentHashMap<>();
  private final AtomicLong nextId = new AtomicLong(0);

  public Router() {
    this.ptr = Ffi.routerNew();
  }

  /** The ABI version the loaded cdylib reports (always 1 for a compatible lib). */
  public static int abiVersion() {
    return 1;
  }

  @Override
  public void close() {
    Ffi.routerFree(ptr);
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
    Abi.AggregateDescriptor.Builder desc =
        Abi.AggregateDescriptor.newBuilder().setName(d.name).setDomain(d.domain);

    for (Map.Entry<String, ApplierThunk> e : d.rebuilder.appliers.entrySet()) {
      long id = assign(applierInvoker(factory, e.getValue()));
      desc.addAppliers(callbackEntry(e.getKey(), id));
    }
    if (d.rebuilder.snapshot != null) {
      desc.setSnapshotCallbackId(assign(applierInvoker(factory, d.rebuilder.snapshot)));
    }
    for (Map.Entry<String, CommandThunk> e : d.commands.entrySet()) {
      long id = assign(commandInvoker(factory, e.getValue()));
      desc.addCommands(callbackEntry(e.getKey(), id));
    }
    for (Map.Entry<String, List<RejectionThunk>> e : d.rejections.entrySet()) {
      Abi.RejectionEntry.Builder entry = Abi.RejectionEntry.newBuilder().setFqCommandType(e.getKey());
      for (RejectionThunk thunk : e.getValue()) {
        entry.addCallbackIds(assign(rejectionInvoker(factory, thunk)));
      }
      desc.addRejections(entry);
    }
    check(Ffi.registerAggregate(ptr, desc.build().toByteArray()));
  }

  public synchronized void registerProjector(ProjectorDispatch d) {
    Supplier<Message.Builder> factory = d.factory;
    Abi.ProjectorDescriptor.Builder desc =
        Abi.ProjectorDescriptor.newBuilder().setName(d.name).addAllDomains(d.domains);

    for (Map.Entry<String, ProjectorEventThunk> e : d.events.entrySet()) {
      long id = assign(projectorEventInvoker(factory, e.getValue()));
      desc.addEvents(callbackEntry(e.getKey(), id));
    }
    if (d.unknown != null) {
      desc.setUnknownCallbackId(assign(projectorUnknownInvoker(d.unknown)));
    }
    if (d.finish != null) {
      desc.setFinishCallbackId(assign(projectorFinishInvoker(factory, d.finish)));
    }
    check(Ffi.registerProjector(ptr, desc.build().toByteArray()));
  }

  public synchronized void registerSaga(SagaDispatch d) {
    Abi.SagaDescriptor.Builder desc =
        Abi.SagaDescriptor.newBuilder()
            .setName(d.name)
            .setInputDomain(d.inputDomain)
            .addAllTargetDomains(d.targets);

    for (Map.Entry<String, SagaEventThunk> e : d.events.entrySet()) {
      long id = assign(sagaEventInvoker(e.getValue()));
      desc.addEvents(callbackEntry(e.getKey(), id));
    }
    for (Map.Entry<String, List<SagaRejectionThunk>> e : d.rejections.entrySet()) {
      Abi.RejectionEntry.Builder entry = Abi.RejectionEntry.newBuilder().setFqCommandType(e.getKey());
      for (SagaRejectionThunk thunk : e.getValue()) {
        entry.addCallbackIds(assign(sagaRejectionInvoker(thunk)));
      }
      desc.addRejections(entry);
    }
    check(Ffi.registerSaga(ptr, desc.build().toByteArray()));
  }

  public synchronized void registerProcessManager(ProcessManagerDispatch d) {
    Supplier<Message.Builder> factory = d.rebuilder.factory;
    Abi.ProcessManagerDescriptor.Builder desc =
        Abi.ProcessManagerDescriptor.newBuilder().setName(d.name).setPmDomain(d.pmDomain);

    for (Map.Entry<String, ApplierThunk> e : d.rebuilder.appliers.entrySet()) {
      long id = assign(applierInvoker(factory, e.getValue()));
      desc.addAppliers(callbackEntry(e.getKey(), id));
    }
    if (d.rebuilder.snapshot != null) {
      desc.setSnapshotCallbackId(assign(applierInvoker(factory, d.rebuilder.snapshot)));
    }
    for (Map.Entry<String, Map<String, PmEventThunk>> byDomain : d.handlers.entrySet()) {
      for (Map.Entry<String, PmEventThunk> e : byDomain.getValue().entrySet()) {
        long id = assign(pmEventInvoker(factory, e.getValue()));
        desc.addEvents(
            Abi.PmEventEntry.newBuilder()
                .setInputDomain(byDomain.getKey())
                .setFqType(e.getKey())
                .setCallbackId(id));
      }
    }
    for (Map.Entry<String, List<PmRejectionThunk>> e : d.rejections.entrySet()) {
      Abi.RejectionEntry.Builder entry = Abi.RejectionEntry.newBuilder().setFqCommandType(e.getKey());
      for (PmRejectionThunk thunk : e.getValue()) {
        entry.addCallbackIds(assign(pmRejectionInvoker(factory, thunk)));
      }
      desc.addRejections(entry);
    }
    check(Ffi.registerProcessManager(ptr, desc.build().toByteArray()));
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
      return call.call(ptr, sessionId, request.toByteArray());
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

  private static Invoker applierInvoker(Supplier<Message.Builder> factory, ApplierThunk thunk) {
    return (session, typeUrl, payload, aux) -> {
      thunk.apply(session.ensureState(factory), anyOf(typeUrl, payload));
      return new Invoker.Result(null, Ffi.STATUS_OK);
    };
  }

  private static Invoker commandInvoker(Supplier<Message.Builder> factory, CommandThunk thunk) {
    return (session, typeUrl, payload, aux) -> {
      Abi.CommandContextAux cax = Abi.CommandContextAux.parseFrom(aux);
      CommandContext cctx =
          new CommandContext(
              Integer.toUnsignedLong(cax.getNextSequence()), cax.getHadPriorEvents());
      EventBook book = thunk.handle(anyOf(typeUrl, payload), session.ensureState(factory), cctx);
      if (book == null) {
        return new Invoker.Result(null, Ffi.STATUS_OK_EMPTY);
      }
      return new Invoker.Result(book.toByteArray(), Ffi.STATUS_OK);
    };
  }

  private static Invoker rejectionInvoker(Supplier<Message.Builder> factory, RejectionThunk thunk) {
    return (session, typeUrl, payload, aux) -> {
      Abi.RejectionAux rax = Abi.RejectionAux.parseFrom(aux);
      Notification n = Notification.parseFrom(rax.getNotification());
      RejectionNotification rej = RejectionNotification.parseFrom(rax.getRejection());
      CommandContext cctx =
          rax.hasCctx()
              ? new CommandContext(
                  Integer.toUnsignedLong(rax.getCctx().getNextSequence()),
                  rax.getCctx().getHadPriorEvents())
              : new CommandContext(0, false);
      BusinessResponse resp = thunk.compensate(n, rej, session.ensureState(factory), cctx);
      if (resp == null) {
        return new Invoker.Result(null, Ffi.STATUS_OK_EMPTY);
      }
      return new Invoker.Result(resp.toByteArray(), Ffi.STATUS_OK);
    };
  }

  private static Invoker projectorEventInvoker(
      Supplier<Message.Builder> factory, ProjectorEventThunk thunk) {
    return (session, typeUrl, payload, aux) -> {
      thunk.fold(session.ensureState(factory), anyOf(typeUrl, payload));
      return new Invoker.Result(null, Ffi.STATUS_OK);
    };
  }

  private static Invoker projectorFinishInvoker(
      Supplier<Message.Builder> factory, ProjectorFinishThunk thunk) {
    return (session, typeUrl, payload, aux) -> {
      EventBook book = EventBook.parseFrom(payload);
      Projection proj = thunk.finish(session.ensureState(factory), book);
      return new Invoker.Result(proj.toByteArray(), Ffi.STATUS_OK);
    };
  }

  private static Invoker projectorUnknownInvoker(ProjectorUnknownThunk thunk) {
    return (session, typeUrl, payload, aux) -> {
      thunk.onUnknown(typeUrl);
      return new Invoker.Result(null, Ffi.STATUS_OK);
    };
  }

  private static Invoker sagaEventInvoker(SagaEventThunk thunk) {
    return (session, typeUrl, payload, aux) -> {
      Abi.SagaEventAux sax = Abi.SagaEventAux.parseFrom(aux);
      Destinations dests = new Destinations(sax.getDestinationSequencesMap());
      SagaEmission emission = thunk.translate(anyOf(typeUrl, payload), dests, sax.getSourceCover());
      SagaResponse resp =
          SagaResponse.newBuilder()
              .addAllCommands(emission.commands())
              .addAllEvents(emission.events())
              .build();
      return new Invoker.Result(resp.toByteArray(), Ffi.STATUS_OK);
    };
  }

  private static Invoker sagaRejectionInvoker(SagaRejectionThunk thunk) {
    return (session, typeUrl, payload, aux) -> {
      Abi.RejectionAux rax = Abi.RejectionAux.parseFrom(aux);
      Notification n = Notification.parseFrom(rax.getNotification());
      RejectionNotification rej = RejectionNotification.parseFrom(rax.getRejection());
      SagaResponse resp = SagaResponse.newBuilder().addAllEvents(thunk.compensate(n, rej)).build();
      return new Invoker.Result(resp.toByteArray(), Ffi.STATUS_OK);
    };
  }

  private static Invoker pmEventInvoker(Supplier<Message.Builder> factory, PmEventThunk thunk) {
    return (session, typeUrl, payload, aux) -> {
      Abi.PmEventAux pax = Abi.PmEventAux.parseFrom(aux);
      Destinations dests = new Destinations(pax.getDestinationSequencesMap());
      ProcessManagerHandleResponse resp =
          thunk.handle(anyOf(typeUrl, payload), session.ensureState(factory), dests);
      return new Invoker.Result(resp.toByteArray(), Ffi.STATUS_OK);
    };
  }

  private static Invoker pmRejectionInvoker(
      Supplier<Message.Builder> factory, PmRejectionThunk thunk) {
    return (session, typeUrl, payload, aux) -> {
      Abi.RejectionAux rax = Abi.RejectionAux.parseFrom(aux);
      Notification n = Notification.parseFrom(rax.getNotification());
      RejectionNotification rej = RejectionNotification.parseFrom(rax.getRejection());
      Thunks.PmRejection r = thunk.compensate(n, rej, session.ensureState(factory));
      ProcessManagerHandleResponse.Builder resp =
          ProcessManagerHandleResponse.newBuilder().addAllProcessEvents(r.processEvents());
      if (r.escalation() != null) {
        resp.setNotification(r.escalation());
      }
      return new Invoker.Result(resp.build().toByteArray(), Ffi.STATUS_OK);
    };
  }
}
