package io.angzarr.router;

import io.angzarr.router.Thunks.PmEventThunk;
import io.angzarr.router.Thunks.PmRejectionThunk;
import java.util.ArrayList;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;

/**
 * One process-manager component's registration: its name, the domain it issues
 * commands to, its rebuilder, per-(source-domain, event) handlers, and ordered
 * rejection compensators. A PM is stateful — its appliers fold process state
 * before a handler runs, exactly as an aggregate does.
 */
public final class ProcessManagerDispatch {
  final String name;
  final String pmDomain;
  final Rebuilder rebuilder;
  // source domain → fully-qualified event type → handler
  final Map<String, Map<String, PmEventThunk>> handlers = new LinkedHashMap<>();
  final Map<String, List<PmRejectionThunk>> rejections = new LinkedHashMap<>();

  public ProcessManagerDispatch(String name, String outputDomain, Rebuilder rebuilder) {
    this.name = name;
    this.pmDomain = outputDomain;
    this.rebuilder = rebuilder;
  }

  /** Registers the handler for one source-domain event type. */
  public ProcessManagerDispatch onEvent(String sourceDomain, String fullName, PmEventThunk thunk) {
    handlers.computeIfAbsent(sourceDomain, k -> new LinkedHashMap<>()).put(fullName, thunk);
    return this;
  }

  /** Appends a compensator for one fully-qualified command type; repeated calls
   * register an ordered fan-out. */
  public ProcessManagerDispatch onRejected(String fqCommand, PmRejectionThunk thunk) {
    rejections.computeIfAbsent(fqCommand, k -> new ArrayList<>()).add(thunk);
    return this;
  }
}
