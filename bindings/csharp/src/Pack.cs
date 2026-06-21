using Google.Protobuf;
using Google.Protobuf.WellKnownTypes;

namespace Angzarr.Router;

/// <summary>
/// Wraps a message in a google.protobuf.Any using the framework's bare-"/"
/// type-URL convention (NOT the type.googleapis.com prefix). The core keys
/// event/command dispatch on it; generated typed-emit wiring uses it to build an
/// EventBook from the typed events a command handler returns.
/// </summary>
public static class Pack
{
    private const string FrameworkAnyPrefix = "/";

    /// <summary>Wraps a message in a bare-"/" Any. Named <c>Wrap</c> (not
    /// <c>Pack</c>) because C# forbids a member sharing its type's name.</summary>
    public static Any Wrap(IMessage msg) =>
        new()
        {
            TypeUrl = FrameworkAnyPrefix + msg.Descriptor.FullName,
            Value = msg.ToByteString(),
        };
}
