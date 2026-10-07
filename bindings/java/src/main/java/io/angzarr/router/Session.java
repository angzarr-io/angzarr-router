package io.angzarr.router;

import com.google.protobuf.Message;
import java.util.HashMap;
import java.util.Map;
import java.util.function.Supplier;

/**
 * One dispatch's host-side state, reached from callbacks via {@code host_ctx}. One dispatch may run
 * several components (co-resident process managers subscribed to the same trigger), so the rebuilt
 * state is keyed per component: every callback a component registered carries that component's key,
 * and the first stateful callback for a key creates its state lazily. State is a {@link
 * Message.Builder}: protobuf-java messages are immutable, so appliers fold events into the builder,
 * and the handler reads it back.
 */
final class Session {
  final Router router;
  private final Map<Long, Message.Builder> states = new HashMap<>();

  Session(Router router) {
    this.router = router;
  }

  /**
   * Returns the state for one component, creating it from the factory on that component's first
   * callback in this dispatch.
   */
  Message.Builder ensureState(long componentKey, Supplier<Message.Builder> factory) {
    return states.computeIfAbsent(componentKey, k -> factory.get());
  }
}
