namespace Angzarr.Router;

/// <summary>
/// The numeric gRPC status code carried with a coded failure. Kept as a plain
/// enum over an int so the binding depends only on the protobuf runtime, not the
/// gRPC library.
/// </summary>
public enum GrpcCode
{
    InvalidArgument = 3,
    NotFound = 5,
    FailedPrecondition = 9,
    Unimplemented = 12,
    Internal = 13,
    DataLoss = 15,
}

/// <summary>Wire-code mapping helpers for <see cref="GrpcCode"/>.</summary>
public static class GrpcCodes
{
    /// <summary>Wire code → GrpcCode; unknown codes degrade to Internal (an
    /// unmapped code is a binding bug).</summary>
    public static GrpcCode FromWire(int code) =>
        code switch
        {
            3 => GrpcCode.InvalidArgument,
            5 => GrpcCode.NotFound,
            9 => GrpcCode.FailedPrecondition,
            12 => GrpcCode.Unimplemented,
            15 => GrpcCode.DataLoss,
            _ => GrpcCode.Internal,
        };
}
