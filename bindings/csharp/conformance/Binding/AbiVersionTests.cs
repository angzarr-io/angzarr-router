using System;
using Angzarr.Router;
using NUnit.Framework;

namespace Angzarr.Router.Conformance.Binding;

/// <summary>The router-ffi ABI version gate.</summary>
[TestFixture]
public sealed class AbiVersionTests
{
    [Test]
    public void TheLoadedLibraryReportsTheExpectedVersion() =>
        Assert.That(Router.AbiVersion(), Is.EqualTo(2));

    [Test]
    public void AMismatchingVersionIsRefusedNamingBothVersions()
    {
        var e = Assert.Throws<InvalidOperationException>(() => Ffi.CheckAbiVersion(1));
        Assert.That(e!.Message, Does.Contain("expected 2"));
        Assert.That(e.Message, Does.Contain("reports 1"));
    }

    [Test]
    public void TheExpectedVersionIsAccepted() =>
        Assert.That(() => Ffi.CheckAbiVersion(Ffi.ExpectedAbiVersion), Throws.Nothing);
}
