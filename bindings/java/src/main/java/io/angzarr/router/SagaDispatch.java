package io.angzarr.router;

import io.angzarr.router.Thunks.SagaEventThunk;
import io.angzarr.router.Thunks.SagaRejectionThunk;
import java.util.ArrayList;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;

/**
 * One saga component's registration: its name, the input domain it consumes,
 * the domains it issues commands to, its event handlers, and ordered rejection
 * compensators. A saga is stateless — no rebuilder, no state.
 */
public final class SagaDispatch {
  final String name;
  final String inputDomain;
  final List<String> targets;
  final Map<String, SagaEventThunk> events = new LinkedHashMap<>();
  final Map<String, List<SagaRejectionThunk>> rejections = new LinkedHashMap<>();

  /** Starts a saga registration translating inputDomain events into commands
   * for targetDomains. */
  public SagaDispatch(String name, String inputDomain, List<String> targetDomains) {
    this.name = name;
    this.inputDomain = inputDomain;
    this.targets = List.copyOf(targetDomains);
  }

  /** Registers the translation thunk for a fully-qualified event type. */
  public SagaDispatch onEvent(String fullName, SagaEventThunk thunk) {
    events.put(fullName, thunk);
    return this;
  }

  /** Appends a compensator for one fully-qualified command type; repeated calls
   * register an ordered fan-out. */
  public SagaDispatch onRejected(String fqCommand, SagaRejectionThunk thunk) {
    rejections.computeIfAbsent(fqCommand, k -> new ArrayList<>()).add(thunk);
    return this;
  }
}
