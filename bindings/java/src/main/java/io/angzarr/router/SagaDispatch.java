package io.angzarr.router;

import io.angzarr.router.Thunks.SagaEventThunk;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;

/**
 * One saga component's registration: its name, the input domain it consumes, the domains it issues
 * commands to, and its event handlers. A saga is a stateless translator — no rebuilder, no state,
 * and no rejections (only aggregates and process managers compensate). Its commands are stamped
 * deferred by the router.
 */
public final class SagaDispatch {
  final String name;
  final String inputDomain;
  final List<String> targets;
  final Map<String, SagaEventThunk> events = new LinkedHashMap<>();

  /** Starts a saga registration translating inputDomain events into commands for targetDomains. */
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
}
