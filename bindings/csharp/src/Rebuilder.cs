using System;
using System.Collections.Generic;
using Google.Protobuf;

namespace Angzarr.Router;

/// <summary>
/// Folds a component's prior events (and optional snapshot) into a state message
/// before a command runs. The factory produces a fresh state message; appliers
/// mutate it page by page. Generic in the state message so appliers stay typed.
/// </summary>
public sealed class Rebuilder<TState>
    where TState : class, IMessage
{
    internal readonly Func<TState> Factory;
    internal readonly Dictionary<string, ApplierThunk<TState>> Appliers = new();
    internal ApplierThunk<TState>? Snapshot;

    /// <summary>Starts a rebuilder from a zero-state factory (e.g.
    /// <c>() => new CounterState()</c>).</summary>
    public Rebuilder(Func<TState> factory) => Factory = factory;

    /// <summary>Registers an applier for one fully-qualified event type.</summary>
    public Rebuilder<TState> Apply(string fullName, ApplierThunk<TState> thunk)
    {
        Appliers[fullName] = thunk;
        return this;
    }

    /// <summary>Registers the snapshot loader that seeds state before pages.</summary>
    public Rebuilder<TState> WithSnapshot(ApplierThunk<TState> thunk)
    {
        Snapshot = thunk;
        return this;
    }
}
