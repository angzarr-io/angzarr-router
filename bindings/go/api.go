package ffirouter

import (
	"errors"

	errdetails "google.golang.org/genproto/googleapis/rpc/errdetails"
	rpcstatus "google.golang.org/genproto/googleapis/rpc/status"
	"google.golang.org/protobuf/proto"
	"google.golang.org/protobuf/types/known/anypb"

	pb "github.com/angzarr-io/angzarr-router/bindings/go/gen/io/angzarr/v1"
)

// codeUnhandledHandlerError is the code an unclassified handler failure
// surfaces as — the binding's job to classify, mirroring client-go.
const codeUnhandledHandlerError = "UNHANDLED_HANDLER_ERROR"

// errorInfoDomain is the reverse-DNS error domain on every ErrorInfo the
// boundary emits (distinct from the io.angzarr proto package).
const errorInfoDomain = "angzarr.io"

// GrpcCode is the numeric gRPC status code carried with a coded error.
// Kept as a plain int32 so the binding depends only on the protobuf
// runtime, not the gRPC library.
type GrpcCode int32

const (
	GrpcInvalidArgument    GrpcCode = 3
	GrpcNotFound           GrpcCode = 5
	GrpcFailedPrecondition GrpcCode = 9
	GrpcUnimplemented      GrpcCode = 12
	GrpcInternal           GrpcCode = 13
	GrpcDataLoss           GrpcCode = 15
)

// CodedError is a stable cross-language coded failure. A handler returns
// one (via Reject) to fail a command with a code like VALUE_NOT_POSITIVE;
// the binding also produces one when decoding a coded failure the core
// returned (NO_HANDLER_REGISTERED, PERSISTED_EVENT_CORRUPT, …). It crosses
// the FFI as google.rpc.Status carrying a google.rpc.ErrorInfo.
type CodedError struct {
	Code    string // SCREAMING_SNAKE cross-language identifier
	Message string
	Grpc    GrpcCode
	Extras  map[string]string
}

func (e *CodedError) Error() string {
	if e.Code != "" {
		return e.Code + ": " + e.Message
	}
	return e.Message
}

// Reject builds an invalid-argument business rejection — the common shape
// a command handler returns to reject the command with a coded reason.
func Reject(code, message string) *CodedError {
	return &CodedError{Code: code, Message: message, Grpc: GrpcInvalidArgument}
}

// CommandContext is the historical-state evidence a handler sees. Host
// state never crosses the FFI, so the core reconstructs this from the
// prior-events book and hands it back — the engine's CommandContext made
// to survive the seam.
type CommandContext struct {
	// NextSequence is the aggregate's next event sequence, derived from the
	// prior-events book.
	NextSequence uint32
	// HadPriorEvents is true when the prior-events book carried any history
	// (pages or snapshot) — the "does this aggregate exist" signal a
	// non-nil zero state cannot convey.
	HadPriorEvents bool
}

// ApplierThunk folds one persisted event into the rebuilding state.
type ApplierThunk[S any] func(state S, payload *anypb.Any) error

// CommandThunk handles one command: it reads the rebuilt state and the
// command context and returns the events to persist (a nil EventBook means
// nothing emitted), or an error — a *CodedError keeps its code, any other
// error becomes UNHANDLED_HANDLER_ERROR.
type CommandThunk[S any] func(cmd *anypb.Any, state S, cctx CommandContext) (*pb.EventBook, error)

// RejectionThunk compensates a rejected command. Multiple thunks for one
// command run in registration order; their responses merge in the core.
type RejectionThunk[S any] func(n *pb.Notification, rejection *pb.RejectionNotification, state S, cctx CommandContext) (*pb.BusinessResponse, error)

// Rebuilder folds an aggregate's prior events (and optional snapshot) into
// state before a command runs.
type Rebuilder[S any] struct {
	factory  func() S
	snapshot ApplierThunk[S]
	appliers map[string]ApplierThunk[S]
}

// NewRebuilder starts a rebuilder from a zero-state factory.
func NewRebuilder[S any](factory func() S) *Rebuilder[S] {
	return &Rebuilder[S]{factory: factory, appliers: make(map[string]ApplierThunk[S])}
}

// Apply registers an applier for one fully-qualified event type.
func (r *Rebuilder[S]) Apply(fullName string, thunk ApplierThunk[S]) *Rebuilder[S] {
	r.appliers[fullName] = thunk
	return r
}

// WithSnapshot registers the snapshot loader that seeds state before pages.
func (r *Rebuilder[S]) WithSnapshot(thunk ApplierThunk[S]) *Rebuilder[S] {
	r.snapshot = thunk
	return r
}

// AggregateDispatch is one aggregate component's registration: its name,
// domain, rebuilder, command handlers, and ordered rejection compensators.
// The shape mirrors the engine's so generated wiring (unit 6) targets it
// with minimal emitter changes.
type AggregateDispatch[S any] struct {
	name       string
	domain     string
	rebuilder  *Rebuilder[S]
	commands   map[string]CommandThunk[S]
	rejections map[string][]RejectionThunk[S]
}

// NewAggregateDispatch starts an aggregate registration.
func NewAggregateDispatch[S any](name, domain string, rebuilder *Rebuilder[S]) *AggregateDispatch[S] {
	return &AggregateDispatch[S]{
		name:       name,
		domain:     domain,
		rebuilder:  rebuilder,
		commands:   make(map[string]CommandThunk[S]),
		rejections: make(map[string][]RejectionThunk[S]),
	}
}

// OnCommand registers a handler for one fully-qualified command type.
func (d *AggregateDispatch[S]) OnCommand(fullName string, thunk CommandThunk[S]) *AggregateDispatch[S] {
	d.commands[fullName] = thunk
	return d
}

// OnRejected appends a compensator for one fully-qualified command type;
// repeated calls register an ordered fan-out.
func (d *AggregateDispatch[S]) OnRejected(fqCommand string, thunk RejectionThunk[S]) *AggregateDispatch[S] {
	d.rejections[fqCommand] = append(d.rejections[fqCommand], thunk)
	return d
}

// buildStatus serializes a coded failure as google.rpc.Status bytes
// carrying a google.rpc.ErrorInfo detail — the exact shape the core
// decodes (and that gRPC puts on the wire).
func buildStatus(grpc GrpcCode, message, code string, extras map[string]string) []byte {
	info := &errdetails.ErrorInfo{Reason: code, Domain: errorInfoDomain, Metadata: extras}
	anyInfo, err := anypb.New(info)
	if err != nil {
		return nil
	}
	st := &rpcstatus.Status{
		Code:    int32(grpc),
		Message: message,
		Details: []*anypb.Any{anyInfo},
	}
	b, err := proto.Marshal(st)
	if err != nil {
		return nil
	}
	return b
}

// errorStatus maps a handler error to (Status bytes, negative gRPC code)
// for the FFI: a *CodedError keeps its code; any other error is an
// unclassified failure → UNHANDLED_HANDLER_ERROR.
func errorStatus(err error) ([]byte, int32) {
	var ce *CodedError
	if errors.As(err, &ce) {
		grpc := ce.Grpc
		if grpc == 0 {
			grpc = GrpcInvalidArgument
		}
		return buildStatus(grpc, ce.Message, ce.Code, ce.Extras), -int32(grpc)
	}
	return buildStatus(GrpcInternal, err.Error(), codeUnhandledHandlerError, nil), -int32(GrpcInternal)
}

// decodeStatus turns google.rpc.Status bytes (with an ErrorInfo detail)
// back into a *CodedError. ret (the negative callback/dispatch return) is
// the gRPC fallback when the bytes are absent or undecodable.
func decodeStatus(b []byte, ret int32) error {
	ce := &CodedError{Grpc: GrpcCode(-ret)}
	var st rpcstatus.Status
	if len(b) > 0 && proto.Unmarshal(b, &st) == nil {
		ce.Message = st.Message
		if st.Code != 0 {
			ce.Grpc = GrpcCode(st.Code)
		}
		for _, d := range st.Details {
			info := &errdetails.ErrorInfo{}
			if d.UnmarshalTo(info) == nil {
				ce.Code = info.Reason
				ce.Extras = info.Metadata
				break
			}
		}
	}
	return ce
}
