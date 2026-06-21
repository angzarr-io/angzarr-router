using System;
using Google.Protobuf;

namespace Angzarr.Router;

/// <summary>
/// One dispatch's host-side state object, reached from callbacks via
/// <c>host_ctx</c>. The rebuilt state is created lazily by the first stateful
/// callback (all callbacks in one dispatch share it). State is a mutable
/// <see cref="IMessage"/> (Google.Protobuf messages are mutable — no Builder);
/// appliers fold events into it and the handler reads it back.
/// </summary>
internal sealed class Session
{
    internal readonly Router Router;
    private IMessage? _state;

    internal Session(Router router) => Router = router;

    /// <summary>Lazily creates the host state from the factory on first callback,
    /// then reuses it across the dispatch.</summary>
    internal IMessage EnsureState(Func<IMessage> factory) => _state ??= factory();
}
