package io.angzarr.router;

import io.angzarr.Cover;

/**
 * The historical-state evidence a handler sees. Host state never crosses the FFI, so the core
 * reconstructs this from the prior-events book and hands it back — the engine's CommandContext made
 * to survive the seam.
 *
 * @param nextSequence the aggregate's next event sequence, derived from the prior-events book
 * @param hadPriorEvents true when the prior-events book carried any history — the "does this
 *     aggregate exist" signal a non-null zero state cannot convey
 * @param cover the cover of the command (or notification delivery) being handled: the aggregate's
 *     own domain and root (the default instance when none was supplied)
 */
public record CommandContext(long nextSequence, boolean hadPriorEvents, Cover cover) {

  /** Normalizes a null cover to the default instance. */
  public CommandContext {
    cover = cover == null ? Cover.getDefaultInstance() : cover;
  }

  /** A context with no cover. */
  public CommandContext(long nextSequence, boolean hadPriorEvents) {
    this(nextSequence, hadPriorEvents, Cover.getDefaultInstance());
  }
}
