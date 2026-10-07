using System;
using System.Collections.Generic;
using Google.Protobuf.WellKnownTypes;

namespace Angzarr.Router;

/// <summary>What a fact handler records: the fact (as received, or annotated)
/// followed by the events that flag it, in order. Each recorded event folds
/// into the state the next fact sees. A fact cannot be refused, so there is no
/// way to record nothing. An <see cref="Any"/> converts implicitly to the fact
/// recorded as received, with no flags.</summary>
public sealed record FactRecord
{
    /// <summary>The fact to record.</summary>
    public Any Fact { get; }

    /// <summary>The events that flag the fact, recorded after it in order.</summary>
    public IReadOnlyList<Any> Flags { get; }

    /// <summary>The fact recorded with the given flagging events.</summary>
    public FactRecord(Any fact, params Any[] flags)
    {
        Fact = fact ?? throw new ArgumentNullException(nameof(fact));
        Flags = Array.AsReadOnly((Any[])flags.Clone());
    }

    /// <summary>The fact recorded as received, with no flags.</summary>
    public static FactRecord AsReceived(Any fact) => new(fact);

    /// <summary>The fact recorded as received, with no flags.</summary>
    public static implicit operator FactRecord(Any fact) => new(fact);
}
