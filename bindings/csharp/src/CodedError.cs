using System;
using System.Collections.Generic;
using Google.Protobuf;
using Google.Protobuf.WellKnownTypes;

namespace Angzarr.Router;

/// <summary>
/// A stable cross-language coded failure. A handler throws one (via
/// <see cref="Reject"/>) to fail a command with a code like
/// <c>VALUE_NOT_POSITIVE</c>; the binding also produces one when decoding a
/// coded failure the core returned. It crosses the FFI as
/// <c>google.rpc.Status</c> carrying a <c>google.rpc.ErrorInfo</c>.
/// </summary>
public sealed class CodedError : Exception
{
    /// <summary>Code an unclassified handler failure surfaces as.</summary>
    public const string UnhandledHandlerError = "UNHANDLED_HANDLER_ERROR";

    /// <summary>Code an Any-decode failure carries.</summary>
    public const string AnyDecodeFailed = "ANY_DECODE_FAILED";

    /// <summary>SCREAMING_SNAKE cross-language identifier (may be empty).</summary>
    public string Code { get; }

    /// <summary>The gRPC status code; never OK — a code of 0 (or below) is
    /// carried as <see cref="GrpcCode.InvalidArgument"/>, since a failure
    /// cannot report success.</summary>
    public GrpcCode Grpc { get; }

    public IReadOnlyDictionary<string, string> Extras { get; }

    public CodedError(
        string? code,
        string? message,
        GrpcCode grpc,
        IReadOnlyDictionary<string, string>? extras
    )
        : base(message ?? "")
    {
        Code = code ?? "";
        Grpc = (int)grpc > 0 ? grpc : GrpcCode.InvalidArgument;
        Extras = extras ?? new Dictionary<string, string>();
    }

    /// <summary>An invalid-argument business rejection — the common shape a
    /// command handler throws to reject a command with a coded reason.</summary>
    public static CodedError Reject(string code, string message) =>
        new(code, message, GrpcCode.InvalidArgument, null);

    /// <summary>Reports that a google.protobuf.Any payload failed to unmarshal —
    /// a malformed payload is an invalid argument, not a handler bug.</summary>
    public static CodedError AnyDecodeError(string typeUrl, Exception cause) =>
        new(
            AnyDecodeFailed,
            "decode Any " + typeUrl + ": " + cause.Message,
            GrpcCode.InvalidArgument,
            new Dictionary<string, string> { ["type_url"] = typeUrl }
        );

    /// <summary>An unclassified failure → UNHANDLED_HANDLER_ERROR / Internal.</summary>
    public static CodedError Unhandled(string message) =>
        new(UnhandledHandlerError, message, GrpcCode.Internal, null);

    /// <summary>
    /// Parses an Any payload into its typed message, mapping a decode failure to
    /// a coded <see cref="AnyDecodeError"/> — the generated dispatch wiring calls
    /// this when unmarshalling a command or event Any.
    /// </summary>
    public static T Parse<T>(MessageParser<T> parser, Any any)
        where T : IMessage<T>
    {
        try
        {
            return parser.ParseFrom(any.Value);
        }
        catch (InvalidProtocolBufferException cause)
        {
            throw AnyDecodeError(any.TypeUrl, cause);
        }
    }
}
