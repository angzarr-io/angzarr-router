namespace Angzarr.Router;

/// <summary>
/// The type-erased bridge from a callback_id to a registered thunk: it receives
/// the per-dispatch <see cref="Session"/> and the marshaled callback inputs and
/// returns the response bytes + ABI status. Throwing is the failure path — the
/// trampoline's single catch is the exception firewall (a thrown exception
/// becomes a coded <c>&lt;0</c> status, never an unwind across the boundary).
/// </summary>
internal delegate InvokerResult Invoker(
    Session session,
    string typeUrl,
    byte[] payload,
    byte[] aux
);

/// <summary>A callback's outcome: response bytes (possibly null/empty) and the
/// ABI status — <c>0</c> ok with payload, <c>1</c> ok with no payload.</summary>
internal readonly record struct InvokerResult(byte[]? Response, int Status);
