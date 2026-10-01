package io.angzarr.router;

import io.angzarr.router.Thunks.CommandThunk;
import io.angzarr.router.Thunks.FactThunk;
import io.angzarr.router.Thunks.RejectionThunk;
import io.angzarr.router.Thunks.UndoThunk;
import java.util.ArrayList;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;

/**
 * One aggregate component's registration: its name, domain, rebuilder, command handlers, ordered
 * rejection compensators, undo handlers and fact handlers. The shape mirrors the engine's so
 * generated wiring targets it with minimal emitter changes.
 */
public final class AggregateDispatch {
  final String name;
  final String domain;
  final Rebuilder rebuilder;
  final Map<String, CommandThunk> commands = new LinkedHashMap<>();
  final Map<String, List<RejectionThunk>> rejections = new LinkedHashMap<>();
  final Map<String, UndoThunk> undoes = new LinkedHashMap<>();
  final Map<String, FactThunk> facts = new LinkedHashMap<>();

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

  /**
   * Appends a compensator for one compensates entry: the rejected command's fully-qualified type
   * ({@code "fq.Type"}, sent to any domain) or {@code "domain:fq.Type"} (only when it was sent to
   * that domain). Repeated calls for one entry register an ordered fan-out.
   */
  public AggregateDispatch onRejected(String compensates, RejectionThunk thunk) {
    rejections.computeIfAbsent(compensates, k -> new ArrayList<>()).add(thunk);
    return this;
  }

  /**
   * Registers the undo handler for one fully-qualified executed command type: a Compensate whose
   * command_type names it routes here.
   */
  public AggregateDispatch onUndo(String fqCommandType, UndoThunk thunk) {
    undoes.put(fqCommandType, thunk);
    return this;
  }

  /**
   * Registers the fact handler for one fully-qualified fact (event) type; a fact with no handler is
   * recorded unchanged.
   */
  public AggregateDispatch onFact(String fqFactType, FactThunk thunk) {
    facts.put(fqFactType, thunk);
    return this;
  }
}
