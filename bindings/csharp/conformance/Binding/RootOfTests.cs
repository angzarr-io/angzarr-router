using System;
using NUnit.Framework;

namespace Angzarr.Router.Conformance.Binding;

/// <summary>Test roots are UUID v5 (NAMESPACE_OID) of their labels, so every
/// language's fixtures carry byte-identical roots.</summary>
[TestFixture]
public sealed class RootOfTests
{
    [Test]
    public void ALabelsRootIsItsUuidV5InTheOidNamespace() =>
        Assert.That(
            Convert.ToHexString(Builders.RootOf("ledger-1")),
            Is.EqualTo("280AA180E8CC5BC197586AEEDA5EC0B0")
        );
}
