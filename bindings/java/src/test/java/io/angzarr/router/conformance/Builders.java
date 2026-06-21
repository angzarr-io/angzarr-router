package io.angzarr.router.conformance;

import com.google.protobuf.Any;
import com.google.protobuf.ByteString;
import com.google.protobuf.InvalidProtocolBufferException;
import com.google.protobuf.Message;
import com.google.protobuf.TextFormat;
import com.google.protobuf.TypeRegistry;
import io.angzarr.CommandBook;
import io.angzarr.CommandPage;
import io.angzarr.ContextualCommand;
import io.angzarr.Cover;
import io.angzarr.EventBook;
import io.angzarr.EventPage;
import io.angzarr.Notification;
import io.angzarr.PageHeader;
import io.angzarr.ProcessManagerHandleRequest;
import io.angzarr.RejectionNotification;
import io.angzarr.SagaHandleRequest;
import io.angzarr.Snapshot;
import io.angzarr.router.Pack;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.Map;
import test.counter.Counter;

/**
 * The shared conformance fixtures — the same orthogonal envelope skeletons
 * (conformance/fixtures/*.txtpb) the Rust cucumber-rs harness parses. Every
 * builder PARSES the skeleton first, then sets the scenario's salient data BY
 * FIELD on the structured message; the textproto is never string-templated.
 * Envelopes that have no skeleton (rejection, prior history, snapshot) are
 * constructed by field, exactly as the Go binding does.
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

  /** Wraps a rejection Notification for fqCommand into a ContextualCommand —
   * the core detects the notification type and takes the compensation path. */
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

  /** An opaque fill-only ext stamped on a command's cover, used to prove ext
   * propagation onto emitted events. */
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

  /** One Increased page whose payload is overwritten with an undecodable varint
   * (PERSISTED_EVENT_CORRUPT on fold). */
  public static EventBook corruptHistory() {
    EventPage page = increasedPageAt(0);
    EventPage corrupt =
        page.toBuilder()
            .setEvent(
                page.getEvent().toBuilder()
                    .setValue(ByteString.copyFrom(new byte[] {(byte) 0xff, (byte) 0xff, (byte) 0xff})))
            .build();
    return EventBook.newBuilder().addPages(corrupt).setNextSequence(1).build();
  }

  /** Seeds count 10 at sequence 10, plus a covered page (10, skipped) and an
   * uncovered page (11, applied) — a rebuild observes 11. */
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

  /** Parses the Increased event skeleton and stamps a sequence. */
  public static EventPage increasedPageAt(int seq) {
    EventPage.Builder page = load("event_increased.txtpb", EventPage.newBuilder());
    page.setHeader(PageHeader.newBuilder().setSequence(seq));
    return page.build();
  }

  // --- saga / process-manager shared fixtures -----------------------------

  /** The one-page Reserve command the saga and PM emit for the "inventory"
   * domain. */
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

  /** A SagaHandleRequest whose source carries one event of fq in the "order"
   * domain, plus the coordinator's destination-sequence map. */
  public static SagaHandleRequest sagaEventSource(String fq, Map<String, Integer> dest) {
    SagaHandleRequest.Builder b =
        SagaHandleRequest.newBuilder()
            .setSource(
                EventBook.newBuilder()
                    .setCover(Cover.newBuilder().setDomain("order"))
                    .addPages(
                        EventPage.newBuilder().setEvent(Any.newBuilder().setTypeUrl(typeUrl(fq)))));
    if (dest != null) {
      b.putAllDestinationSequences(dest);
    }
    return b.build();
  }

  /** A SagaHandleRequest whose source is a rejection Notification for fqCommand
   * — routes to the compensation path. */
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

  /** A request whose trigger carries the given event pages in domain, plus the
   * PM's prior state and a destination map. Trigger event pages are built by
   * field (the fq list includes types with no skeleton, e.g. Unwatched). */
  public static ProcessManagerHandleRequest pmTrigger(
      String domain, java.util.List<String> fqs, EventBook state, Map<String, Integer> dest) {
    EventBook.Builder trigger = EventBook.newBuilder().setCover(Cover.newBuilder().setDomain(domain));
    for (String fq : fqs) {
      trigger.addPages(EventPage.newBuilder().setEvent(Any.newBuilder().setTypeUrl(typeUrl(fq))));
    }
    ProcessManagerHandleRequest.Builder b =
        ProcessManagerHandleRequest.newBuilder().setTrigger(trigger);
    if (state != null) {
      b.setProcessState(state);
    }
    if (dest != null) {
      b.putAllDestinationSequences(dest);
    }
    return b.build();
  }

  /** A prior-state book of n Increased events (drives the rebuild). */
  public static EventBook pmStateOf(int n) {
    EventBook.Builder book = EventBook.newBuilder();
    for (int i = 0; i < n; i++) {
      book.addPages(increasedEventPage());
    }
    return book.build();
  }

  /** A trigger whose newest page is a rejection Notification for fqCommand. */
  public static ProcessManagerHandleRequest pmRejection(String fqCommand) {
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
    return ProcessManagerHandleRequest.newBuilder()
        .setTrigger(
            EventBook.newBuilder()
                .setCover(Cover.newBuilder().setDomain("counter"))
                .addPages(EventPage.newBuilder().setEvent(Pack.pack(notification))))
        .build();
  }

  public static ProcessManagerHandleRequest pmNoTrigger() {
    return ProcessManagerHandleRequest.getDefaultInstance();
  }

  public static ProcessManagerHandleRequest pmEmptyTrigger() {
    return ProcessManagerHandleRequest.newBuilder().setTrigger(EventBook.getDefaultInstance()).build();
  }
}
