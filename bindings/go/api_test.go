package ffirouter

import (
	"errors"
	"testing"

	pb "github.com/angzarr-io/angzarr-router/bindings/go/gen/io/angzarr/v1"
)

func TestPack_UsesFrameworkBareSlashTypeURL(t *testing.T) {
	any, err := Pack(&pb.Notification{})
	if err != nil {
		t.Fatalf("Pack: %v", err)
	}
	if want := "/io.angzarr.v1.Notification"; any.TypeUrl != want {
		t.Errorf("TypeUrl = %q, want %q (bare-slash, not type.googleapis.com)", any.TypeUrl, want)
	}
}

func TestAnyDecodeError_IsInvalidArgumentWithTypeURL(t *testing.T) {
	err := AnyDecodeError("/io.angzarr.v1.Notification", errors.New("boom"))
	if err.Code != codeAnyDecodeFailed {
		t.Errorf("Code = %q, want %q", err.Code, codeAnyDecodeFailed)
	}
	if err.Grpc != GrpcInvalidArgument {
		t.Errorf("Grpc = %v, want InvalidArgument", err.Grpc)
	}
	if err.Extras["type_url"] != "/io.angzarr.v1.Notification" {
		t.Errorf("Extras[type_url] = %q, want the type URL", err.Extras["type_url"])
	}
}
