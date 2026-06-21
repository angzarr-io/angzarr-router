package io.angzarr.router;

import io.angzarr.router.Thunks.CommandThunk;
import io.angzarr.router.Thunks.RejectionThunk;
import java.util.ArrayList;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;

/**
 * One aggregate component's registration: its name, domain, rebuilder, command
 * handlers, and ordered rejection compensators. The shape mirrors the engine's
 * so generated wiring targets it with minimal emitter changes.
 */
public final class AggregateDispatch {
  final String name;
  final String domain;
  final Rebuilder rebuilder;
  final Map<String, CommandThunk> commands = new LinkedHashMap<>();
  final Map<String, List<RejectionThunk>> rejections = new LinkedHashMap<>();

  public AggregateDispatch(String name, String domain, Rebuilder rebuilder) {
    this.name = name;
    this.domain = domain;
    this.rebuilder = rebuilder;
  }

  /** Registers a handler for one fully-qualified command type. */
  public AggregateDispatch onCommand(String fullName, CommandThunk thunk) {
    commands.put(fullName, thunk);
    return this;
  }

  /** Appends a compensator for one fully-qualified command type; repeated calls
   * register an ordered fan-out. */
  public AggregateDispatch onRejected(String fqCommand, RejectionThunk thunk) {
    rejections.computeIfAbsent(fqCommand, k -> new ArrayList<>()).add(thunk);
    return this;
  }
}
