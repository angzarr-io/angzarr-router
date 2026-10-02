using System;
using System.Collections.Generic;
using Google.Protobuf;

namespace Angzarr.Router;

/// <summary>
/// One dispatch's host-side state, reached from callbacks via <c>host_ctx</c>.
/// A single dispatch can run several components (e.g. co-resident process
/// managers subscribed to the same trigger), so rebuilt state is keyed per
/// registered component: each component's callbacks share one state object,
/// created lazily by its first stateful callback, and never see another
/// component's state. State is a mutable <see cref="IMessage"/>
/// (Google.Protobuf messages are mutable — no Builder); appliers fold events
/// into it and the handler reads it back.
/// </summary>
internal sealed class Session
{
    internal readonly Router Router;
    private readonly Dictionary<ComponentKey, IMessage> _states = new();

    internal Session(Router router) => Router = router;

    /// <summary>Returns the state of the component identified by
    /// <paramref name="component"/>, creating it from <paramref name="factory"/>
    /// on that component's first callback in this dispatch.</summary>
    internal IMessage EnsureState(ComponentKey component, Func<IMessage> factory)
    {
        if (!_states.TryGetValue(component, out var state))
        {
            state = factory();
            _states[component] = state;
        }
        return state;
    }
}

/// <summary>Identity of one registered component; every invoker registered for
/// that component captures the same key, so they share its per-dispatch
/// state.</summary>
internal sealed class ComponentKey { }
