package io.angzarr.router;

import com.google.protobuf.Any;
import com.google.protobuf.Message;
import io.angzarr.BusinessResponse;
import io.angzarr.CommandBook;
import io.angzarr.Compensate;
import io.angzarr.Cover;
import io.angzarr.EventBook;
import io.angzarr.Notification;
import io.angzarr.ProcessManagerHandleResponse;
import io.angzarr.Projection;
import io.angzarr.RejectionNotification;
import java.util.List;

/**
 * The typed business thunks the dispatch builders hold and the generated wiring provides. State is
 * a {@link Message.Builder} (protobuf-java is immutable; appliers fold events into the builder);
 * the generated lambdas cast it to the concrete builder type. A thunk throws to fail — the
 * trampoline catches and codes it.
 */
public final class Thunks {
  private Thunks() {}

  @FunctionalInterface
  public interface ApplierThunk {
    void apply(Message.Builder state, Any event) throws Exception;
  }

  @FunctionalInterface
  public interface CommandThunk {
    /** Returns the EventBook to persist, or null for nothing emitted. */
    EventBook handle(Any command, Message.Builder state, CommandContext cctx) throws Exception;
  }

  @FunctionalInterface
  public interface RejectionThunk {
    /** Returns a BusinessResponse, or null for nothing. */
    BusinessResponse compensate(
        Notification notification,
        RejectionNotification rejection,
        Message.Builder state,
        CommandContext cctx)
        throws Exception;
  }

  @FunctionalInterface
  public interface UndoThunk {
    /**
     * Undoes an executed command named by the Compensate; returns a BusinessResponse, or null for
     * nothing.
     */
    BusinessResponse undo(
        Notification notification,
        Compensate compensate,
        Message.Builder state,
        CommandContext cctx)
        throws Exception;
  }

  @FunctionalInterface
  public interface FactThunk {
    /**
     * Returns the fact to record (an annotated replacement), or null to record it unchanged. State
     * is the aggregate's rebuilt state.
     */
    Any handle(Any fact, Message.Builder state) throws Exception;
  }

  @FunctionalInterface
  public interface ProjectorEventThunk {
    void fold(Message.Builder projection, Any event) throws Exception;
  }

  @FunctionalInterface
  public interface ProjectorEventContextThunk {
    /** ctx is where the event sits: its book's cover and the page's sequence. */
    void fold(Message.Builder projection, Any event, PageContext ctx) throws Exception;
  }

  @FunctionalInterface
  public interface ProjectorFinishThunk {
    Projection finish(Message.Builder projection, EventBook events) throws Exception;
  }

  @FunctionalInterface
  public interface ProjectorUnknownThunk {
    void onUnknown(String typeUrl);
  }

  @FunctionalInterface
  public interface SagaEventThunk {
    /**
     * sourceCover is the source book's cover, so the saga can route emitted commands by the
     * trigger's identity (root, ext).
     */
    SagaEmission translate(Any event, Destinations dests, Cover sourceCover) throws Exception;
  }

  @FunctionalInterface
  public interface PmEventThunk {
    ProcessManagerHandleResponse handle(Any event, Message.Builder state, Destinations dests)
        throws Exception;
  }

  @FunctionalInterface
  public interface PmEventCoverThunk {
    /** triggerCover is the trigger book's cover. */
    ProcessManagerHandleResponse handle(
        Any event, Message.Builder state, Destinations dests, Cover triggerCover) throws Exception;
  }

  @FunctionalInterface
  public interface PmRejectionThunk {
    PmRejection compensate(
        Notification notification, RejectionNotification rejection, Message.Builder state)
        throws Exception;
  }

  @FunctionalInterface
  public interface PmCompensatorThunk {
    /**
     * Returns the full response (process events, commands, facts, escalation), or null for nothing.
     */
    ProcessManagerHandleResponse compensate(
        Notification notification, RejectionNotification rejection, Message.Builder state)
        throws Exception;
  }

  /** A saga event's emission: commands to issue + fact events to inject. */
  public record SagaEmission(List<CommandBook> commands, List<EventBook> events) {}

  /**
   * A PM rejection's result: process events to fold + an optional escalation notification (null for
   * none).
   */
  public record PmRejection(List<EventBook> processEvents, Notification escalation) {}
}
