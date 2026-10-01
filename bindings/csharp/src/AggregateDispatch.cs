using System.Collections.Generic;
using Google.Protobuf;

namespace Angzarr.Router;

/// <summary>
/// One aggregate component's registration: its name, domain, rebuilder, command
/// handlers, ordered rejection compensators, undo handlers and fact handlers.
/// Generic in the state message so handler thunks see the concrete state — the
/// generated wiring is cast-free. The router packs the rebuilt state for Replay
/// on its own (the state is a protobuf message).
/// </summary>
public sealed class AggregateDispatch<TState>
    where TState : class, IMessage
{
    internal readonly string Name;
    internal readonly string Domain;
    internal readonly Rebuilder<TState> Rebuilder;
    internal readonly Dictionary<string, CommandThunk<TState>> Commands = new();
    internal readonly Dictionary<string, List<RejectionThunk<TState>>> Rejections = new();
    internal readonly Dictionary<string, UndoThunk<TState>> Undoes = new();
    internal readonly Dictionary<string, FactThunk<TState>> Facts = new();

    public AggregateDispatch(string name, string domain, Rebuilder<TState> rebuilder)
    {
        Name = name;
        Domain = domain;
        Rebuilder = rebuilder;
    }

    /// <summary>Registers a handler for one fully-qualified command type.</summary>
    public AggregateDispatch<TState> OnCommand(string fullName, CommandThunk<TState> thunk)
    {
        Commands[fullName] = thunk;
        return this;
    }

    /// <summary>Appends a compensator for one compensates entry: the rejected
    /// command's fully-qualified type (<c>"fq.Type"</c>, sent to any domain) or
    /// <c>"domain:fq.Type"</c> (only when it was sent to that domain). The entry
    /// passes to the router verbatim; repeated calls register an ordered
    /// fan-out.</summary>
    public AggregateDispatch<TState> OnRejected(string compensates, RejectionThunk<TState> thunk)
    {
        if (!Rejections.TryGetValue(compensates, out var list))
        {
            list = new List<RejectionThunk<TState>>();
            Rejections[compensates] = list;
        }
        list.Add(thunk);
        return this;
    }

    /// <summary>Registers the undo handler for one fully-qualified executed
    /// command type: a Compensate whose <c>command_type</c> names it routes
    /// here. A Compensate with no undo handler is refused as
    /// <c>NO_UNDO_HANDLER</c> (UNIMPLEMENTED).</summary>
    public AggregateDispatch<TState> OnUndo(string fqCommandType, UndoThunk<TState> thunk)
    {
        Undoes[fqCommandType] = thunk;
        return this;
    }

    /// <summary>Registers the fact handler for one fully-qualified fact (event)
    /// type. A fact with no handler is recorded unchanged.</summary>
    public AggregateDispatch<TState> OnFact(string fqFactType, FactThunk<TState> thunk)
    {
        Facts[fqFactType] = thunk;
        return this;
    }
}
