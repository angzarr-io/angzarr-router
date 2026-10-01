using Angzarr;
using NUnit.Framework;

namespace Angzarr.Router.Conformance;

/// <summary>Assertions shared by several feature step classes.</summary>
internal static class Steps
{
    /// <summary>Every page of <paramref name="cmd"/> is deferred from the
    /// triggering page: angzarr_deferred with the trigger book's cover domain,
    /// its sequence, and the command's emission index — never an explicit
    /// sequence.</summary>
    internal static void AssertDeferred(CommandBook cmd, string sourceDomain, int seq, int index)
    {
        Assert.That(cmd.Pages, Is.Not.Empty, "command pages");
        foreach (var page in cmd.Pages)
        {
            Assert.That(
                page.Header?.SequenceTypeCase,
                Is.EqualTo(PageHeader.SequenceTypeOneofCase.AngzarrDeferred),
                "command page is deferred"
            );
            var d = page.Header!.AngzarrDeferred;
            Assert.That((int)d.SourceSeq, Is.EqualTo(seq), "source_seq is the trigger's");
            Assert.That((int)d.CommandIndex, Is.EqualTo(index), "command_index");
            Assert.That(d.Source?.Domain, Is.EqualTo(sourceDomain), "source cover");
        }
    }
}
