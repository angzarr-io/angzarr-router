package ffirouter

import (
	"errors"
	"strings"
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

func TestCheckAbiVersion_AcceptsTheExpectedVersion(t *testing.T) {
	if err := checkAbiVersion(ExpectedAbiVersion); err != nil {
		t.Fatalf("checkAbiVersion(%d) = %v, want nil", ExpectedAbiVersion, err)
	}
}

func TestCheckAbiVersion_RefusesADriftedLibraryNamingBothVersions(t *testing.T) {
	err := checkAbiVersion(ExpectedAbiVersion + 1)
	if err == nil {
		t.Fatal("checkAbiVersion accepted a mismatched ABI version")
	}
	msg := err.Error()
	if !strings.Contains(msg, "expects ABI version 1") || !strings.Contains(msg, "reports ABI version 2") {
		t.Errorf("error %q does not name expected (1) and actual (2) versions", msg)
	}
}
