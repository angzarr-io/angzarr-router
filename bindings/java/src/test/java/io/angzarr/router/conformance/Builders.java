package io.angzarr.router.conformance;

import com.google.protobuf.Any;
import com.google.protobuf.ByteString;
import com.google.protobuf.InvalidProtocolBufferException;
import com.google.protobuf.Message;
import com.google.protobuf.TextFormat;
import com.google.protobuf.TypeRegistry;
import io.angzarr.AngzarrDeferredSequence;
import io.angzarr.BusinessResponse;
import io.angzarr.CommandBook;
import io.angzarr.CommandPage;
import io.angzarr.Compensate;
import io.angzarr.ContextualCommand;
import io.angzarr.Cover;
import io.angzarr.EventBook;
import io.angzarr.EventPage;
import io.angzarr.FactRequest;
import io.angzarr.Notification;
import io.angzarr.PageHeader;
import io.angzarr.ProcessManagerHandleRequest;
import io.angzarr.RejectionNotification;
import io.angzarr.ReplayRequest;
import io.angzarr.SagaHandleRequest;
import io.angzarr.Snapshot;
import io.angzarr.router.Pack;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.List;
import test.counter.Counter;

/**
 * The shared conformance fixtures — the same orthogonal envelope skeletons
 * (conformance/fixtures/*.txtpb) the Rust cucumber-rs harness parses. Every builder PARSES the
 * skeleton first, then sets the scenario's salient data BY FIELD on the structured message; the
 * textproto is never string-templated. Envelopes that have no skeleton (rejection, prior history,
 * snapshot) are constructed by field, exactly as the Go binding does.
 */
public final class Builders {
  private Builders() {}

  private static final Path DIR = Path.of("../../conformance/fixtures");

  private static final TextFormat.Parser PARSER =
      TextFormat.Parser.newBuilder()
          .setTypeRegistry(
              TypeRegistry.newBuilder().add(Counter.getDescriptor().getMessageTypes()).build())
          .build();

  /** The framework's canonical bare-"/" type-URL prefix. */
  public static String typeUrl(String fq) {
    return "/" + fq;
  }

  public static <B extends Message.Builder> B load(String name, B builder) {
    try {
      PARSER.merge(Files.readString(DIR.resolve(name)), builder);
      return builder;
    } catch (Exception e) {
      throw new IllegalStateException("load fixture " + name + ": " + e, e);
    }
  }

  // --- commands -----------------------------------------------------------

  /** Parses the IncreaseBy skeleton, then sets n on the inner message by field. */
  public static ContextualCommand increaseCommand(int n) {
    ContextualCommand.Builder cc = load("command_increase.txtpb", ContextualCommand.newBuilder());
    CommandPage.Builder page = cc.getCommandBuilder().getPagesBuilder(0);
    Any any = page.getCommand();
    try {
      Counter.IncreaseBy inner =
          Counter.IncreaseBy.parseFrom(any.getValue()).toBuilder().setN(n).build();
      page.setCommand(any.toBuilder().setValue(inner.toByteString()));
    } catch (InvalidProtocolBufferException e) {
      throw new IllegalStateException("decode IncreaseBy skeleton", e);
    }
    return cc.build();
  }

  /** A parsed increase command with parent linkage stamped on its cover. */
  public static ContextualCommand increaseCommandWithLinkage(int n) {
    ContextualCommand.Builder cc = increaseCommand(n).toBuilder();
    cc.getCommandBuilder().getCoverBuilder().setExt(parentLinkage());
    return cc.build();
  }

  public static ContextualCommand failHardCommand() {
    return load("command_failhard.txtpb", ContextualCommand.newBuilder()).build();
  }

  public static ContextualCommand unhandledCommand() {
    return load("command_unhandled.txtpb", ContextualCommand.newBuilder()).build();
  }

  /**
   * Wraps a rejection Notification for fqCommand into a ContextualCommand — the core detects the
   * notification type and takes the compensation path.
   */
  public static ContextualCommand rejectionCommand(String fqCommand) {
    Cover cover = Cover.newBuilder().setDomain("counter").build();
    RejectionNotification rejection =
        RejectionNotification.newBuilder()
            .setRejectedCommand(
                CommandBook.newBuilder()
                    .setCover(cover)
                    .addPages(
                        CommandPage.newBuilder()
                            .setCommand(Any.newBuilder().setTypeUrl(typeUrl(fqCommand)))))
            .build();
    Notification notification = Notification.newBuilder().setPayload(Pack.pack(rejection)).build();
    return ContextualCommand.newBuilder()
        .setCommand(
            CommandBook.newBuilder()
                .setCover(cover)
                .addPages(CommandPage.newBuilder().setCommand(Pack.pack(notification))))
        .build();
  }

  // --- envelope-guard negatives (one structural field cleared) ------------

  public static ContextualCommand commandMissingBook() {
    return increaseCommand(1).toBuilder().clearCommand().build();
  }

  public static ContextualCommand commandMissingPage() {
    ContextualCommand.Builder cc = increaseCommand(1).toBuilder();
    cc.getCommandBuilder().clearPages();
    return cc.build();
  }

  public static ContextualCommand commandMissingPayload() {
    ContextualCommand.Builder cc = increaseCommand(1).toBuilder();
    cc.getCommandBuilder().getPagesBuilder(0).clearPayload();
    return cc.build();
  }

  /**
   * An opaque fill-only ext stamped on a command's cover, used to prove ext propagation onto
   * emitted events.
   */
  public static Any parentLinkage() {
    return Any.newBuilder()
        .setTypeUrl(typeUrl("test.counter.Parent"))
        .setValue(ByteString.copyFrom(new byte[] {1, 2, 3}))
        .build();
  }

  // --- prior history ------------------------------------------------------

  /** Replays the Increased skeleton at consecutive sequences 0..n-1 (null if 0). */
  public static EventBook priorIncreases(int n) {
    if (n == 0) {
      return null;
    }
    EventBook.Builder book = EventBook.newBuilder().setNextSequence(n);
    for (int i = 0; i < n; i++) {
      book.addPages(increasedPageAt(i));
    }
    return book.build();
  }

  /**
   * One Increased page whose payload is overwritten with an undecodable varint
   * (PERSISTED_EVENT_CORRUPT on fold).
   */
  public static EventBook corruptHistory() {
    EventPage page = increasedPageAt(0);
    EventPage corrupt =
        page.toBuilder()
            .setEvent(
                page.getEvent().toBuilder()
                    .setValue(
                        ByteString.copyFrom(new byte[] {(byte) 0xff, (byte) 0xff, (byte) 0xff})))
            .build();
    return EventBook.newBuilder().addPages(corrupt).setNextSequence(1).build();
  }

  /**
   * Seeds count 10 at sequence 10, plus a covered page (10, skipped) and an uncovered page (11,
   * applied) — a rebuild observes 11.
   */
  public static EventBook snapshotHistory() {
    return EventBook.newBuilder()
        .setSnapshot(
            Snapshot.newBuilder()
                .setSequence(10)
                .setState(Pack.pack(Counter.CounterState.newBuilder().setCount(10).build())))
        .addPages(increasedPageAt(10))
        .addPages(increasedPageAt(11))
        .setNextSequence(12)
        .build();
  }

  /**
   * Rewrites every Any type URL in the command and its prior history to prefix + the
   * fully-qualified name.
   */
  public static ContextualCommand withTypeUrlPrefix(ContextualCommand cmd, String prefix) {
    ContextualCommand.Builder b = cmd.toBuilder();
    if (b.hasCommand()) {
      for (CommandPage.Builder page : b.getCommandBuilder().getPagesBuilderList()) {
        if (page.hasCommand()) {
          page.setCommand(reprefix(page.getCommand(), prefix));
        }
      }
    }
    if (b.hasEvents()) {
      b.setEvents(withTypeUrlPrefix(b.getEvents(), prefix));
    }
    return b.build();
  }

  /** Rewrites every event Any type URL in the book to prefix + the fully-qualified name. */
  public static EventBook withTypeUrlPrefix(EventBook book, String prefix) {
    EventBook.Builder b = book.toBuilder();
    for (EventPage.Builder page : b.getPagesBuilderList()) {
      if (page.hasEvent()) {
        page.setEvent(reprefix(page.getEvent(), prefix));
      }
    }
    return b.build();
  }

  private static Any reprefix(Any any, String prefix) {
    return any.toBuilder().setTypeUrl(prefix + fqOf(any.getTypeUrl())).build();
  }

  /** The fully-qualified type name of a type URL: everything after the last "/". */
  public static String fqOf(String typeUrl) {
    return typeUrl.substring(typeUrl.lastIndexOf('/') + 1);
  }

  /** Parses the Increased event skeleton and stamps a sequence. */
  public static EventPage increasedPageAt(int seq) {
    EventPage.Builder page = load("event_increased.txtpb", EventPage.newBuilder());
    page.setHeader(PageHeader.newBuilder().setSequence(seq));
    return page.build();
  }

  // --- saga / process-manager shared fixtures -----------------------------

  /** The one-page Reserve command the saga and PM emit for the "inventory" domain. */
  public static CommandBook reserveCommand() {
    return CommandBook.newBuilder()
        .setCover(Cover.newBuilder().setDomain("inventory"))
        .addPages(
            CommandPage.newBuilder()
                .setCommand(Any.newBuilder().setTypeUrl(typeUrl("test.counter.Reserve"))))
        .build();
  }

  /** A single empty fact-event book the compensators inject. */
  public static EventBook oneFact() {
    return EventBook.newBuilder().addPages(EventPage.newBuilder()).build();
  }

  // --- saga dispatch requests (no skeleton — built by field) --------------

  /**
   * A SagaHandleRequest whose source carries one event of fq in the "order" domain, at sequence seq
   * when given (null: no header).
   */
  public static SagaHandleRequest sagaEventSource(String fq, Integer seq) {
    EventPage.Builder page =
        EventPage.newBuilder().setEvent(Any.newBuilder().setTypeUrl(typeUrl(fq)));
    if (seq != null) {
      page.setHeader(PageHeader.newBuilder().setSequence(seq));
    }
    return SagaHandleRequest.newBuilder()
        .setSource(
            EventBook.newBuilder().setCover(Cover.newBuilder().setDomain("order")).addPages(page))
        .build();
  }

  /** A SagaHandleRequest whose source carries one Increased event of order root label at seq. */
  public static SagaHandleRequest sagaRootedSource(String label, int seq) {
    SagaHandleRequest.Builder req = sagaEventSource("test.counter.Increased", seq).toBuilder();
    req.getSourceBuilder().setCover(coverOf("order", label));
    return req.build();
  }

  /** count bytes counting up from first. */
  private static ByteString byteRun(int first, int count) {
    byte[] b = new byte[count];
    for (int i = 0; i < count; i++) {
      b[i] = (byte) (first + i);
    }
    return ByteString.copyFrom(b);
  }

  /**
   * The parity command: cover "inventory", root bytes 10..1f, correlation "corr-1"; one page whose
   * command is "/example.Foo" carrying 01020304.
   */
  public static CommandBook parityCommand() {
    return CommandBook.newBuilder()
        .setCover(
            Cover.newBuilder()
                .setDomain("inventory")
                .setRoot(io.angzarr.UUID.newBuilder().setValue(byteRun(0x10, 16)))
                .setCorrelationId("corr-1"))
        .addPages(
            CommandPage.newBuilder()
                .setCommand(Any.newBuilder().setTypeUrl("/example.Foo").setValue(byteRun(1, 4))))
        .build();
  }

  /**
   * The parity source: one Increased event at sequence seq under cover "order", root bytes 00..0f,
   * correlation "corr-1".
   */
  public static SagaHandleRequest paritySource(int seq) {
    SagaHandleRequest.Builder req = sagaEventSource("test.counter.Increased", seq).toBuilder();
    req.getSourceBuilder()
        .setCover(
            Cover.newBuilder()
                .setDomain("order")
                .setRoot(io.angzarr.UUID.newBuilder().setValue(byteRun(0x00, 16)))
                .setCorrelationId("corr-1"));
    return req.build();
  }

  /**
   * A SagaHandleRequest whose source is a rejection Notification for fqCommand — a saga skips it
   * (sagas receive no rejections).
   */
  public static SagaHandleRequest sagaRejectionSource(String fqCommand) {
    RejectionNotification rejection =
        RejectionNotification.newBuilder()
            .setRejectedCommand(
                CommandBook.newBuilder()
                    .setCover(Cover.newBuilder().setDomain("inventory"))
                    .addPages(
                        CommandPage.newBuilder()
                            .setCommand(Any.newBuilder().setTypeUrl(typeUrl(fqCommand)))))
            .build();
    Notification notification = Notification.newBuilder().setPayload(Pack.pack(rejection)).build();
    return SagaHandleRequest.newBuilder()
        .setSource(
            EventBook.newBuilder()
                .setCover(Cover.newBuilder().setDomain("order"))
                .addPages(EventPage.newBuilder().setEvent(Pack.pack(notification))))
        .build();
  }

  public static SagaHandleRequest sagaSourceNoPages() {
    return SagaHandleRequest.newBuilder().setSource(EventBook.getDefaultInstance()).build();
  }

  public static SagaHandleRequest sagaRequestNoSource() {
    return SagaHandleRequest.getDefaultInstance();
  }

  // --- projector deliveries -----------------------------------------------

  /** One Increased event page (no header) from the shared skeleton. */
  public static EventPage increasedEventPage() {
    return load("event_increased.txtpb", EventPage.newBuilder()).build();
  }

  /** An EventBook of n Increased events whose cover carries domain. */
  public static EventBook deliveryBook(String domain, int n) {
    EventBook.Builder book = EventBook.newBuilder().setCover(Cover.newBuilder().setDomain(domain));
    for (int i = 0; i < n; i++) {
      book.addPages(increasedEventPage());
    }
    return book.build();
  }

  public static EventBook deliveryNoCover() {
    return deliveryBook("counter", 1).toBuilder().clearCover().build();
  }

  // --- process-manager triggers (no skeleton — built by field) ------------

  /**
   * A request whose trigger carries the given event pages in domain, the newest at sequence
   * newestSeq when given (null: no header), plus the PM's prior state. Trigger event pages are
   * built by field (the fq list includes types with no skeleton, e.g. Unwatched).
   */
  public static ProcessManagerHandleRequest pmTrigger(
      String domain, List<String> fqs, EventBook state, Integer newestSeq) {
    EventBook.Builder trigger =
        EventBook.newBuilder().setCover(Cover.newBuilder().setDomain(domain));
    for (String fq : fqs) {
      trigger.addPages(EventPage.newBuilder().setEvent(Any.newBuilder().setTypeUrl(typeUrl(fq))));
    }
    if (newestSeq != null && trigger.getPagesCount() > 0) {
      trigger
          .getPagesBuilder(trigger.getPagesCount() - 1)
          .setHeader(PageHeader.newBuilder().setSequence(newestSeq));
    }
    ProcessManagerHandleRequest.Builder b =
        ProcessManagerHandleRequest.newBuilder().setTrigger(trigger);
    if (state != null) {
      b.setProcessState(state);
    }
    return b.build();
  }

  /**
   * A PM request whose trigger (in the order PM's own domain) is a Compensate for an executed
   * test.counter.&lt;command&gt;.
   */
  public static ProcessManagerHandleRequest pmCompensate(String command) {
    Notification notification =
        Notification.newBuilder().setPayload(compensatePayload(command)).build();
    return ProcessManagerHandleRequest.newBuilder()
        .setTrigger(
            EventBook.newBuilder()
                .setCover(Cover.newBuilder().setDomain("order-pm"))
                .addPages(EventPage.newBuilder().setEvent(Pack.pack(notification))))
        .build();
  }

  /** The Compensate payload for an executed test.counter.&lt;command&gt;. */
  public static Any compensatePayload(String command) {
    return Pack.pack(
        Compensate.newBuilder()
            .setCommandType("test.counter." + command)
            .addSequences(0)
            .setReason("aborted")
            .build());
  }

  /** The root bytes for a label: UUID v5 in the OID namespace. */
  public static byte[] rootOf(String label) {
    return Uuid5.oid(label);
  }

  /** A cover in domain with the root for label. */
  public static Cover coverOf(String domain, String label) {
    return Cover.newBuilder()
        .setDomain(domain)
        .setRoot(io.angzarr.UUID.newBuilder().setValue(ByteString.copyFrom(rootOf(label))))
        .build();
  }

  /** A page header carrying an explicit sequence. */
  public static PageHeader sequenceHeader(int seq) {
    return PageHeader.newBuilder().setSequence(seq).build();
  }

  /** A prior-state book of n Increased events (drives the rebuild). */
  public static EventBook pmStateOf(int n) {
    EventBook.Builder book = EventBook.newBuilder();
    for (int i = 0; i < n; i++) {
      book.addPages(increasedEventPage());
    }
    return book.build();
  }

  /**
   * A prior-state book of n Increased events owned by pmDomain (its cover addresses the owning PM).
   */
  public static EventBook pmStateIn(String pmDomain, int n) {
    return pmStateOf(n).toBuilder().setCover(Cover.newBuilder().setDomain(pmDomain)).build();
  }

  /** A trigger whose newest page is a rejection Notification for fqCommand. */
  public static ProcessManagerHandleRequest pmRejection(String fqCommand) {
    return pmRejectionRequest(fqCommand, "counter", PageHeader.getDefaultInstance());
  }

  /**
   * A rejection of fqCommand that the PM owning issuerDomain issued: the trigger cover is the
   * issuer's domain and the rejected command's first page header names the issuer as its
   * angzarr_deferred source.
   */
  public static ProcessManagerHandleRequest pmIssuedRejection(
      String fqCommand, String issuerDomain) {
    PageHeader header =
        PageHeader.newBuilder()
            .setAngzarrDeferred(
                AngzarrDeferredSequence.newBuilder()
                    .setSource(Cover.newBuilder().setDomain(issuerDomain)))
            .build();
    return pmRejectionRequest(fqCommand, issuerDomain, header);
  }

  private static ProcessManagerHandleRequest pmRejectionRequest(
      String fqCommand, String triggerDomain, PageHeader commandHeader) {
    CommandPage.Builder page =
        CommandPage.newBuilder().setCommand(Any.newBuilder().setTypeUrl(typeUrl(fqCommand)));
    if (!commandHeader.equals(PageHeader.getDefaultInstance())) {
      page.setHeader(commandHeader);
    }
    RejectionNotification rejection =
        RejectionNotification.newBuilder()
            .setRejectedCommand(
                CommandBook.newBuilder()
                    .setCover(Cover.newBuilder().setDomain("inventory"))
                    .addPages(page))
            .build();
    Notification notification = Notification.newBuilder().setPayload(Pack.pack(rejection)).build();
    return ProcessManagerHandleRequest.newBuilder()
        .setTrigger(
            EventBook.newBuilder()
                .setCover(Cover.newBuilder().setDomain(triggerDomain))
                .addPages(EventPage.newBuilder().setEvent(Pack.pack(notification))))
        .build();
  }

  // --- compensation routing (compensation.feature) ------------------------

  /**
   * A Notification command (bare "/" type URL) wrapping payload, addressed to domain, over prior
   * history whose next sequence is nextSequence when given (null: no prior history).
   */
  public static ContextualCommand notificationCommand(
      String domain, Any payload, Integer nextSequence) {
    Notification notification = Notification.newBuilder().setPayload(payload).build();
    ContextualCommand.Builder cc =
        ContextualCommand.newBuilder()
            .setCommand(
                CommandBook.newBuilder()
                    .setCover(Cover.newBuilder().setDomain(domain))
                    .addPages(CommandPage.newBuilder().setCommand(Pack.pack(notification))));
    if (nextSequence != null) {
      cc.setEvents(
          EventBook.newBuilder()
              .setNextSequence(nextSequence)
              .addPages(
                  EventPage.newBuilder()
                      .setHeader(sequenceHeader(Math.max(0, nextSequence - 1)))
                      .setEvent(Any.newBuilder().setTypeUrl(typeUrl("test.counter.Unrelated")))));
    }
    return cc.build();
  }

  /**
   * The rejection of a test.counter.&lt;command&gt; sent to targetDomain, delivered to the payment
   * aggregate over prior history ending before nextSequence when given.
   */
  public static ContextualCommand rejectionSentTo(
      String command, String targetDomain, Integer nextSequence) {
    RejectionNotification rejection =
        RejectionNotification.newBuilder()
            .setRejectedCommand(
                CommandBook.newBuilder()
                    .setCover(Cover.newBuilder().setDomain(targetDomain))
                    .addPages(
                        CommandPage.newBuilder()
                            .setCommand(
                                Any.newBuilder().setTypeUrl(typeUrl("test.counter." + command)))))
            .build();
    return notificationCommand("payment", Pack.pack(rejection), nextSequence);
  }

  /**
   * A Compensate for an executed test.counter.&lt;command&gt;, delivered to the inventory
   * aggregate.
   */
  public static ContextualCommand compensateFor(String command) {
    return notificationCommand("inventory", compensatePayload(command), null);
  }

  /** A business response carrying one header-less event page of test.counter.&lt;name&gt;. */
  public static BusinessResponse oneEvent(String name) {
    return BusinessResponse.newBuilder()
        .setEvents(
            EventBook.newBuilder()
                .addPages(
                    EventPage.newBuilder()
                        .setEvent(Any.newBuilder().setTypeUrl(typeUrl("test.counter." + name)))))
        .build();
  }

  // --- facts, replay and handler context (context.feature) ----------------

  /** An Increased page (empty payload) at sequence seq. */
  public static EventPage increasedAt(int seq) {
    return EventPage.newBuilder()
        .setHeader(sequenceHeader(seq))
        .setEvent(Pack.pack(Counter.Increased.getDefaultInstance()))
        .build();
  }

  /**
   * A FactRequest of facts pages of test.counter.&lt;fact&gt; in "ledger" over prior Increased
   * events.
   */
  public static FactRequest factRequest(String fact, int facts, int prior) {
    EventBook.Builder book =
        EventBook.newBuilder().setCover(Cover.newBuilder().setDomain("ledger"));
    for (int i = 0; i < facts; i++) {
      book.addPages(
          EventPage.newBuilder()
              .setEvent(Any.newBuilder().setTypeUrl(typeUrl("test.counter." + fact))));
    }
    EventBook.Builder history = EventBook.newBuilder().setNextSequence(prior);
    for (int i = 0; i < prior; i++) {
      history.addPages(increasedAt(i));
    }
    return FactRequest.newBuilder().setFacts(book).setPriorEvents(history).build();
  }

  /** A ReplayRequest: a snapshot of count at sequence 1, then events Increased at sequences 2... */
  public static ReplayRequest replayRequest(int count, int events) {
    ReplayRequest.Builder req =
        ReplayRequest.newBuilder()
            .setBaseSnapshot(
                Snapshot.newBuilder()
                    .setSequence(1)
                    .setState(
                        Pack.pack(Counter.CounterState.newBuilder().setCount(count).build())));
    for (int i = 0; i < events; i++) {
      req.addEvents(increasedAt(2 + i));
    }
    return req.build();
  }

  /** A ReplayRequest of events Increased events at sequences 0..., with no snapshot. */
  public static ReplayRequest eventsReplayRequest(int events) {
    ReplayRequest.Builder req = ReplayRequest.newBuilder();
    for (int i = 0; i < events; i++) {
      req.addEvents(increasedAt(i));
    }
    return req.build();
  }

  /** An IncreaseBy command for the ledger root label. */
  public static ContextualCommand ledgerCommand(String label) {
    return ContextualCommand.newBuilder()
        .setCommand(
            CommandBook.newBuilder()
                .setCover(coverOf("ledger", label))
                .addPages(
                    CommandPage.newBuilder()
                        .setCommand(Pack.pack(Counter.IncreaseBy.newBuilder().setN(1).build()))))
        .build();
  }

  /** The parent linkage the ledger sets on its own events. */
  public static Any ledgerLinkage() {
    return Any.newBuilder()
        .setTypeUrl(typeUrl("test.counter.Parent"))
        .setValue(ByteString.copyFrom(new byte[] {4, 5, 6}))
        .build();
  }

  /** An IncreaseBy command for the ledger root label on behalf of a parent. */
  public static ContextualCommand ledgerCommandWithLinkage(String label) {
    ContextualCommand.Builder cc = ledgerCommand(label).toBuilder();
    cc.getCommandBuilder().getCoverBuilder().setExt(parentLinkage());
    return cc.build();
  }

  /** An Increased trigger from "counter" root label at sequence seq. */
  public static ProcessManagerHandleRequest reservingTrigger(String label, int seq) {
    return ProcessManagerHandleRequest.newBuilder()
        .setTrigger(
            EventBook.newBuilder().setCover(coverOf("counter", label)).addPages(increasedAt(seq)))
        .build();
  }

  /**
   * The rejection of a Reserve sent to targetDomain, delivered to the reserving process-manager's
   * own domain at sequence seq.
   */
  public static ProcessManagerHandleRequest reservingRejection(String targetDomain, int seq) {
    RejectionNotification rejection =
        RejectionNotification.newBuilder()
            .setRejectedCommand(
                reserveCommand().toBuilder().setCover(Cover.newBuilder().setDomain(targetDomain)))
            .build();
    Notification notification = Notification.newBuilder().setPayload(Pack.pack(rejection)).build();
    return ProcessManagerHandleRequest.newBuilder()
        .setTrigger(
            EventBook.newBuilder()
                .setCover(Cover.newBuilder().setDomain("reserving-pm"))
                .addPages(
                    EventPage.newBuilder()
                        .setHeader(sequenceHeader(seq))
                        .setEvent(Pack.pack(notification))))
        .build();
  }

  /** A book of Increased events of "counter" root label at sequences. */
  public static EventBook trackedBook(String label, int... sequences) {
    EventBook.Builder book = EventBook.newBuilder().setCover(coverOf("counter", label));
    for (int seq : sequences) {
      book.addPages(increasedAt(seq));
    }
    return book.build();
  }

  public static ProcessManagerHandleRequest pmNoTrigger() {
    return ProcessManagerHandleRequest.getDefaultInstance();
  }

  public static ProcessManagerHandleRequest pmEmptyTrigger() {
    return ProcessManagerHandleRequest.newBuilder()
        .setTrigger(EventBook.getDefaultInstance())
        .build();
  }
}
