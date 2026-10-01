namespace Angzarr.Router;

/// <summary>
/// Type-URL helpers. An Any is identified by the fully-qualified message name
/// after the last <c>/</c>; the resolver prefix before it (<c>/</c>,
/// <c>type.googleapis.com/</c>, or any other) is immaterial.
/// </summary>
public static class TypeNames
{
    /// <summary>The fully-qualified message name a type URL carries: everything
    /// after its last <c>/</c> (the whole string when it has none).</summary>
    public static string FromUrl(string typeUrl)
    {
        var i = typeUrl.LastIndexOf('/');
        return i >= 0 ? typeUrl[(i + 1)..] : typeUrl;
    }
}
