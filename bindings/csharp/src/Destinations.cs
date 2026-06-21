using System.Collections.Generic;
using System.Linq;

namespace Angzarr.Router;

/// <summary>
/// The coordinator-supplied next-sequences for command stamping. Sagas and
/// process managers are translators — they stamp emitted commands, they do not
/// rebuild destination state to make decisions.
/// </summary>
public sealed class Destinations
{
    private readonly IReadOnlyDictionary<string, uint> _sequences;

    /// <summary>Wraps a domain→next-sequence map (null becomes empty).</summary>
    public Destinations(IReadOnlyDictionary<string, uint>? sequences)
    {
        _sequences = sequences ?? new Dictionary<string, uint>();
    }

    /// <summary>Returns the next sequence for a domain, or null if none exists.</summary>
    public uint? SequenceFor(string domain) =>
        _sequences.TryGetValue(domain, out var seq) ? seq : null;

    /// <summary>Reports whether a sequence exists for the domain.</summary>
    public bool Has(string domain) => _sequences.ContainsKey(domain);

    /// <summary>Every domain carrying a sequence (unordered).</summary>
    public IReadOnlyList<string> Domains() => _sequences.Keys.ToList();

    /// <summary>
    /// Returns a copy of <paramref name="cmd"/> with every page stamped with the
    /// next sequence for <paramref name="domain"/>. A domain with no supplied
    /// sequence is the coded <c>MISSING_DESTINATION_SEQUENCE</c>.
    /// </summary>
    public CommandBook StampCommand(CommandBook cmd, string domain)
    {
        if (!_sequences.TryGetValue(domain, out var seq))
        {
            throw new CodedError(
                "MISSING_DESTINATION_SEQUENCE",
                "no sequence for destination domain",
                GrpcCode.InvalidArgument,
                new Dictionary<string, string> { ["domain"] = domain }
            );
        }
        var book = cmd.Clone();
        foreach (var page in book.Pages)
        {
            page.Header = new PageHeader { Sequence = seq };
        }
        return book;
    }
}
