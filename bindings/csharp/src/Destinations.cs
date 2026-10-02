using System.Collections.Generic;
using System.Linq;

namespace Angzarr.Router;

/// <summary>
/// The output domains a saga or process manager declares (its command targets).
/// Emitted commands are deferred: they carry no destination sequence, and the
/// router stamps their <c>angzarr_deferred</c> provenance from the triggering
/// page, so a handler never stamps a command and never needs destination state.
/// </summary>
public sealed class Destinations
{
    private readonly IReadOnlyList<string> _domains;

    /// <summary>The declared output domains, in declaration order (null becomes
    /// none).</summary>
    public Destinations(IEnumerable<string>? domains)
    {
        _domains = domains?.ToList() ?? new List<string>();
    }

    /// <summary>Reports whether <paramref name="domain"/> is a declared output
    /// domain.</summary>
    public bool Has(string domain) => _domains.Contains(domain);

    /// <summary>The declared output domains, in declaration order.</summary>
    public IReadOnlyList<string> Domains() => _domains;
}
