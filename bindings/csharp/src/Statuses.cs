using System;
using System.Collections.Generic;
using Google.Protobuf;
using Google.Protobuf.WellKnownTypes;
using Google.Rpc;

namespace Angzarr.Router;

/// <summary>
/// Serializes a <see cref="CodedError"/> as <c>google.rpc.Status</c> bytes
/// carrying a <c>google.rpc.ErrorInfo</c> detail — the exact shape the core
/// decodes (and that gRPC puts on the wire) — and back.
/// </summary>
internal static class Statuses
{
    private const string ErrorInfoDomain = "angzarr.io";
    private const string ErrorInfoTypeUrl = "type.googleapis.com/google.rpc.ErrorInfo";

    internal static byte[] ToStatusBytes(CodedError err)
    {
        var info = new ErrorInfo { Reason = err.Code, Domain = ErrorInfoDomain };
        foreach (var kv in err.Extras)
        {
            info.Metadata.Add(kv.Key, kv.Value);
        }
        var status = new Google.Rpc.Status { Code = (int)err.Grpc, Message = err.Message ?? "" };
        status.Details.Add(new Any { TypeUrl = ErrorInfoTypeUrl, Value = info.ToByteString() });
        return status.ToByteArray();
    }

    /// <summary>Maps any exception to (Status bytes, negative gRPC code): a
    /// CodedError keeps its code; anything else is UNHANDLED_HANDLER_ERROR /
    /// Internal.</summary>
    internal static InvokerResult ErrorResult(Exception t)
    {
        var ce =
            t as CodedError
            ?? CodedError.Unhandled(string.IsNullOrEmpty(t.Message) ? t.ToString() : t.Message);
        return new InvokerResult(ToStatusBytes(ce), -(int)ce.Grpc);
    }

    /// <summary>Decodes google.rpc.Status bytes into a CodedError; <paramref
    /// name="ret"/> (the negative callback/dispatch return) is the gRPC fallback
    /// when the bytes are absent or undecodable.</summary>
    internal static CodedError FromStatusBytes(byte[]? bytes, int ret)
    {
        var fallback = GrpcCodes.FromWire(-ret);
        if (bytes == null || bytes.Length == 0)
        {
            return new CodedError(
                "",
                "host callback failed without a status payload",
                fallback,
                null
            );
        }
        Google.Rpc.Status status;
        try
        {
            status = Google.Rpc.Status.Parser.ParseFrom(bytes);
        }
        catch (InvalidProtocolBufferException)
        {
            return CodedError.Unhandled("host callback returned undecodable status bytes");
        }
        var code = "";
        IReadOnlyDictionary<string, string> extras = new Dictionary<string, string>();
        foreach (var detail in status.Details)
        {
            if (detail.TypeUrl == ErrorInfoTypeUrl)
            {
                try
                {
                    var info = ErrorInfo.Parser.ParseFrom(detail.Value);
                    code = info.Reason;
                    extras = new Dictionary<string, string>(info.Metadata);
                }
                catch (InvalidProtocolBufferException)
                {
                    // leave code/extras empty — the message + fallback still classify it
                }
                break;
            }
        }
        var grpc = status.Code != 0 ? GrpcCodes.FromWire(status.Code) : fallback;
        return new CodedError(code, status.Message, grpc, extras);
    }
}
