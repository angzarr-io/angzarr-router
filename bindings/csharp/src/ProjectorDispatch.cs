using System;
using System.Collections.Generic;
using System.Linq;
using Google.Protobuf;

namespace Angzarr.Router;

/// <summary>
/// One projector component's registration: its name, projection-state factory,
/// the domains it folds, per-event fold thunks, and the finisher that carries
/// the cover onto the Projection. Generic in the projection state message.
/// </summary>
public sealed class ProjectorDispatch<TState>
    where TState : class, IMessage
{
    internal readonly string Name;
    internal readonly Func<TState> Factory;
    internal IReadOnlyList<string> Domains = Array.Empty<string>();
    internal readonly Dictionary<string, ProjectorEventThunk<TState>> Events = new();
    internal ProjectorFinishThunk<TState>? FinishThunk;
    internal ProjectorUnknownThunk? Unknown;

    public ProjectorDispatch(string name, Func<TState> factory)
    {
        Name = name;
        Factory = factory;
    }

    /// <summary>Declares the domains this projector folds.</summary>
    public ProjectorDispatch<TState> ForDomains(params string[] domains)
    {
        Domains = domains.ToList();
        return this;
    }

    /// <summary>Registers the fold thunk for a fully-qualified event type.</summary>
    public ProjectorDispatch<TState> OnEvent(string fullName, ProjectorEventThunk<TState> thunk)
    {
        Events[fullName] = thunk;
        return this;
    }

    /// <summary>Registers the finisher that produces the Projection from the
    /// folded state.</summary>
    public ProjectorDispatch<TState> Finish(ProjectorFinishThunk<TState> thunk)
    {
        FinishThunk = thunk;
        return this;
    }

    /// <summary>Registers an optional observer for events outside the declared
    /// set.</summary>
    public ProjectorDispatch<TState> OnUnknown(ProjectorUnknownThunk thunk)
    {
        Unknown = thunk;
        return this;
    }
}
