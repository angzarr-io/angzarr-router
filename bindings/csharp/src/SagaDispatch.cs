using System.Collections.Generic;
using System.Linq;

namespace Angzarr.Router;

/// <summary>
/// One saga component's registration: its name, the input domain it consumes,
/// the domains it issues commands to, its event handlers, and ordered rejection
/// compensators. A saga is stateless — no rebuilder, no state.
/// </summary>
public sealed class SagaDispatch
{
    internal readonly string Name;
    internal readonly string InputDomain;
    internal readonly IReadOnlyList<string> Targets;
    internal readonly Dictionary<string, SagaEventThunk> Events = new();
    internal readonly Dictionary<string, List<SagaRejectionThunk>> Rejections = new();

    /// <summary>Starts a saga registration translating inputDomain events into
    /// commands for targetDomains.</summary>
    public SagaDispatch(string name, string inputDomain, params string[] targetDomains)
    {
        Name = name;
        InputDomain = inputDomain;
        Targets = targetDomains.ToList();
    }

    /// <summary>Registers the translation thunk for a fully-qualified event type.</summary>
    public SagaDispatch OnEvent(string fullName, SagaEventThunk thunk)
    {
        Events[fullName] = thunk;
        return this;
    }

    /// <summary>Appends a compensator for one fully-qualified command type;
    /// repeated calls register an ordered fan-out.</summary>
    public SagaDispatch OnRejected(string fqCommand, SagaRejectionThunk thunk)
    {
        if (!Rejections.TryGetValue(fqCommand, out var list))
        {
            list = new List<SagaRejectionThunk>();
            Rejections[fqCommand] = list;
        }
        list.Add(thunk);
        return this;
    }
}
