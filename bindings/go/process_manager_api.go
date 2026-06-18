package ffirouter

import (
	"google.golang.org/protobuf/types/known/anypb"

	pb "github.com/angzarr-io/angzarr-router/bindings/go/gen/io/angzarr/v1"
)

// PMEventThunk handles the newest trigger event against rebuilt PM state,
// returning the full response (process events, commands, facts, optional
// escalation). Binding/generated thunks unmarshal to the typed event and call
// the typed business method.
type PMEventThunk[S any] func(event *anypb.Any, state S, dests *Destinations) (*pb.ProcessManagerHandleResponse, error)

// PMRejectionThunk compensates a rejected PM-issued command against rebuilt
// state, returning process events and an optional escalation Notification.
// Multiple thunks for one command run in registration order (C-0042); their
// process events merge and the first escalation wins.
type PMRejectionThunk[S any] func(n *pb.Notification, rejection *pb.RejectionNotification, state S) ([]*pb.EventBook, *pb.Notification, error)

// ProcessManagerDispatch is one process-manager component's registration: its
// name, its own domain, the rebuilder for its event-sourced state, event
// handlers keyed by (input domain, FQ event type), and ordered rejection
// compensators. The shape mirrors the core's so generated wiring (unit 6)
// targets it with minimal emitter changes.
type ProcessManagerDispatch[S any] struct {
	name       string
	pmDomain   string
	rebuilder  *Rebuilder[S]
	handlers   map[string]map[string]PMEventThunk[S]
	rejections map[string][]PMRejectionThunk[S]
}

// NewProcessManagerDispatch starts a PM registration over a Rebuilder for the
// PM's own event-sourced state.
func NewProcessManagerDispatch[S any](name, pmDomain string, rebuilder *Rebuilder[S]) *ProcessManagerDispatch[S] {
	return &ProcessManagerDispatch[S]{
		name:       name,
		pmDomain:   pmDomain,
		rebuilder:  rebuilder,
		handlers:   make(map[string]map[string]PMEventThunk[S]),
		rejections: make(map[string][]PMRejectionThunk[S]),
	}
}

// OnEvent registers the thunk for (input domain, fully-qualified event type).
func (d *ProcessManagerDispatch[S]) OnEvent(inputDomain, fullName string, thunk PMEventThunk[S]) *ProcessManagerDispatch[S] {
	if d.handlers[inputDomain] == nil {
		d.handlers[inputDomain] = make(map[string]PMEventThunk[S])
	}
	d.handlers[inputDomain][fullName] = thunk
	return d
}

// OnRejected appends a compensator for one fully-qualified command type;
// repeated calls register an ordered fan-out (C-0042).
func (d *ProcessManagerDispatch[S]) OnRejected(fqCommand string, thunk PMRejectionThunk[S]) *ProcessManagerDispatch[S] {
	d.rejections[fqCommand] = append(d.rejections[fqCommand], thunk)
	return d
}
