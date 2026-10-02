namespace Angzarr.Router;

/// <summary>
/// The historical-state evidence and identity a handler sees. Host state never
/// crosses the FFI, so the core reconstructs this from the prior-events book and
/// hands it back — the engine's CommandContext made to survive the seam.
/// </summary>
/// <param name="NextSequence">The aggregate's next event sequence, derived from
/// the prior-events book.</param>
/// <param name="HadPriorEvents">True when the prior-events book carried any
/// history — the "does this aggregate exist" signal a zero state cannot
/// convey.</param>
/// <param name="Cover">The cover of the command (or notification delivery)
/// being handled: the aggregate's own domain and root. Null when the core
/// supplied none.</param>
public readonly record struct CommandContext(
    long NextSequence,
    bool HadPriorEvents,
    Cover? Cover = null
);
