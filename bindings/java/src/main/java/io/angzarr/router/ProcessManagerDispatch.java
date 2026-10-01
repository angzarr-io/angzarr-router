package io.angzarr.router;

import io.angzarr.ProcessManagerHandleResponse;
import io.angzarr.router.Thunks.PmCompensatorThunk;
import io.angzarr.router.Thunks.PmEventCoverThunk;
import io.angzarr.router.Thunks.PmEventThunk;
import io.angzarr.router.Thunks.PmRejectionThunk;
import java.util.ArrayList;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;

/**
 * One process-manager component's registration: its name, its own domain, the output domains it
 * issues commands to, its rebuilder, per-(source-domain, event) handlers, and ordered rejection
 * compensators. A PM is stateful — its appliers fold process state before a handler runs, exactly
 * as an aggregate does. Its commands are stamped deferred by the router.
 */
public final class ProcessManagerDispatch {
  final String name;
  final String pmDomain;
  final List<String> targets;
  final Rebuilder rebuilder;
  // source domain → fully-qualified event type → handler
  final Map<String, Map<String, PmEventCoverThunk>> handlers = new LinkedHashMap<>();
  final Map<String, List<PmCompensatorThunk>> rejections = new LinkedHashMap<>();

  /** Starts a PM registration in pmDomain declaring no output domains. */
  public ProcessManagerDispatch(String name, String pmDomain, Rebuilder rebuilder) {
    this(name, pmDomain, List.of(), rebuilder);
  }

  /** Starts a PM registration in pmDomain issuing commands to targetDomains. */
  public ProcessManagerDispatch(
      String name, String pmDomain, List<String> targetDomains, Rebuilder rebuilder) {
    this.name = name;
    this.pmDomain = pmDomain;
    this.targets = List.copyOf(targetDomains);
    this.rebuilder = rebuilder;
  }

  /** Registers the handler for one source-domain event type. */
  public ProcessManagerDispatch onEvent(String sourceDomain, String fullName, PmEventThunk thunk) {
    return onEvent(
        sourceDomain,
        fullName,
        (event, state, dests, triggerCover) -> thunk.handle(event, state, dests));
  }

  /**
   * Registers the handler for one source-domain event type that also reads the trigger book's
   * cover.
   */
  public ProcessManagerDispatch onEvent(
      String sourceDomain, String fullName, PmEventCoverThunk thunk) {
    handlers.computeIfAbsent(sourceDomain, k -> new LinkedHashMap<>()).put(fullName, thunk);
    return this;
  }

  /**
   * Appends a compensator for one compensates entry ({@code "fq.Type"} or {@code "domain:fq.Type"})
   * returning process events and an optional escalation; repeated calls register an ordered
   * fan-out.
   */
  public ProcessManagerDispatch onRejected(String compensates, PmRejectionThunk thunk) {
    return onRejectedResponse(
        compensates,
        (n, rejection, state) -> {
          Thunks.PmRejection r = thunk.compensate(n, rejection, state);
          ProcessManagerHandleResponse.Builder resp =
              ProcessManagerHandleResponse.newBuilder().addAllProcessEvents(r.processEvents());
          if (r.escalation() != null) {
            resp.setNotification(r.escalation());
          }
          return resp.build();
        });
  }

  /**
   * Appends a compensator for one compensates entry returning a full response (process events,
   * commands — stamped deferred by the router — facts, escalation); repeated calls register an
   * ordered fan-out.
   */
  public ProcessManagerDispatch onRejectedResponse(String compensates, PmCompensatorThunk thunk) {
    rejections.computeIfAbsent(compensates, k -> new ArrayList<>()).add(thunk);
    return this;
  }
}
