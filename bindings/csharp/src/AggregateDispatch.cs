using System.Collections.Generic;
using Google.Protobuf;

namespace Angzarr.Router;

/// <summary>
/// One aggregate component's registration: its name, domain, rebuilder, command
/// handlers, and ordered rejection compensators. Generic in the state message so
/// handler thunks see the concrete state — the generated wiring is cast-free.
/// </summary>
public sealed class AggregateDispatch<TState>
    where TState : class, IMessage
{
    internal readonly string Name;
    internal readonly string Domain;
    internal readonly Rebuilder<TState> Rebuilder;
    internal readonly Dictionary<string, CommandThunk<TState>> Commands = new();
    internal readonly Dictionary<string, List<RejectionThunk<TState>>> Rejections = new();

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

    /// <summary>Appends a compensator for one fully-qualified command type;
    /// repeated calls register an ordered fan-out.</summary>
    public AggregateDispatch<TState> OnRejected(string fqCommand, RejectionThunk<TState> thunk)
    {
        if (!Rejections.TryGetValue(fqCommand, out var list))
        {
            list = new List<RejectionThunk<TState>>();
            Rejections[fqCommand] = list;
        }
        list.Add(thunk);
        return this;
    }
}
