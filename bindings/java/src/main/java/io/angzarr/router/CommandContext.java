package io.angzarr.router;

/**
 * The historical-state evidence a handler sees. Host state never crosses the
 * FFI, so the core reconstructs this from the prior-events book and hands it
 * back — the engine's CommandContext made to survive the seam.
 *
 * @param nextSequence the aggregate's next event sequence, derived from the
 *     prior-events book
 * @param hadPriorEvents true when the prior-events book carried any history —
 *     the "does this aggregate exist" signal a non-null zero state cannot convey
 */
public record CommandContext(long nextSequence, boolean hadPriorEvents) {}
