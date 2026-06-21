using System.Collections.Generic;
using Google.Protobuf;

namespace Angzarr.Router;

/// <summary>
/// One process-manager component's registration: its name, the domain it issues
/// commands to, its rebuilder, per-(source-domain, event) handlers, and ordered
/// rejection compensators. A PM is stateful — its appliers fold process state
/// before a handler runs, exactly as an aggregate does. Generic in the state.
/// </summary>
public sealed class ProcessManagerDispatch<TState>
    where TState : class, IMessage
{
    internal readonly string Name;
    internal readonly string PmDomain;
    internal readonly Rebuilder<TState> Rebuilder;

    // source domain → fully-qualified event type → handler
    internal readonly Dictionary<string, Dictionary<string, PmEventThunk<TState>>> Handlers = new();
    internal readonly Dictionary<string, List<PmRejectionThunk<TState>>> Rejections = new();

    public ProcessManagerDispatch(string name, string outputDomain, Rebuilder<TState> rebuilder)
    {
        Name = name;
        PmDomain = outputDomain;
        Rebuilder = rebuilder;
    }

    /// <summary>Registers the handler for one source-domain event type.</summary>
    public ProcessManagerDispatch<TState> OnEvent(
        string sourceDomain,
        string fullName,
        PmEventThunk<TState> thunk
    )
    {
        if (!Handlers.TryGetValue(sourceDomain, out var byType))
        {
            byType = new Dictionary<string, PmEventThunk<TState>>();
            Handlers[sourceDomain] = byType;
        }
        byType[fullName] = thunk;
        return this;
    }

    /// <summary>Appends a compensator for one fully-qualified command type;
    /// repeated calls register an ordered fan-out.</summary>
    public ProcessManagerDispatch<TState> OnRejected(
        string fqCommand,
        PmRejectionThunk<TState> thunk
    )
    {
        if (!Rejections.TryGetValue(fqCommand, out var list))
        {
            list = new List<PmRejectionThunk<TState>>();
            Rejections[fqCommand] = list;
        }
        list.Add(thunk);
        return this;
    }
}
