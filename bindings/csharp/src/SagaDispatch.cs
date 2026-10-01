using System.Collections.Generic;
using System.Linq;

namespace Angzarr.Router;

/// <summary>
/// One saga component's registration: its name, the input domain it consumes,
/// the domains it issues commands to, and its event handlers. A saga is a
/// stateless translator — no rebuilder, no state, and no rejections (only
/// aggregates and process managers compensate).
/// </summary>
public sealed class SagaDispatch
{
    internal readonly string Name;
    internal readonly string InputDomain;
    internal readonly IReadOnlyList<string> Targets;
    internal readonly Dictionary<string, SagaEventPageThunk> Events = new();

    /// <summary>Starts a saga registration translating inputDomain events into
    /// commands for targetDomains.</summary>
    public SagaDispatch(string name, string inputDomain, params string[] targetDomains)
    {
        Name = name;
        InputDomain = inputDomain;
        Targets = targetDomains.ToList();
    }

    /// <summary>Registers the translation thunk for a fully-qualified event type.</summary>
    public SagaDispatch OnEvent(string fullName, SagaEventThunk thunk) =>
        OnEventWithContext(fullName, (ev, dests, source) => thunk(ev, dests, source.Cover!));

    /// <summary>Registers the translation thunk for a fully-qualified event
    /// type; the thunk also reads where the triggering event sits
    /// (<see cref="PageContext"/>: its book's cover and the event's
    /// sequence).</summary>
    public SagaDispatch OnEventWithContext(string fullName, SagaEventPageThunk thunk)
    {
        Events[fullName] = thunk;
        return this;
    }
}
