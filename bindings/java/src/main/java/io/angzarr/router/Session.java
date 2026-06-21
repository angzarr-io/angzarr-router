package io.angzarr.router;

import com.google.protobuf.Message;
import java.util.function.Supplier;

/**
 * One dispatch's host-side state object, reached from callbacks via {@code
 * host_ctx}. The rebuilt state is created lazily by the first stateful callback
 * (all callbacks in one dispatch share it). State is a {@link Message.Builder}:
 * protobuf-java messages are immutable, so appliers fold events into the
 * builder, and the command/event handler reads it back.
 */
final class Session {
  final Router router;
  private Message.Builder state;

  Session(Router router) {
    this.router = router;
  }

  /** Lazily creates the host state from the factory on first callback, then
   * reuses it across the dispatch. */
  Message.Builder ensureState(Supplier<Message.Builder> factory) {
    if (state == null) {
      state = factory.get();
    }
    return state;
  }
}
