package main

import (
	"net/http"
	"net/http/httptest"
	"testing"
)

func okHandler() http.Handler {
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.WriteHeader(http.StatusOK)
	})
}

func callWithAuth(t *testing.T, token, header string) int {
	t.Helper()
	h := withAuth(token, okHandler())
	req := httptest.NewRequest(http.MethodGet, "http://example.test/api/state", nil)
	if header != "" {
		req.Header.Set("Authorization", header)
	}
	rec := httptest.NewRecorder()
	h.ServeHTTP(rec, req)
	return rec.Code
}

func TestBearer가맞으면통과(t *testing.T) {
	if code := callWithAuth(t, "secret123", "Bearer secret123"); code != http.StatusOK {
		t.Fatalf("코드 = %d, 기대 200", code)
	}
}

func TestBearer가틀리면거부(t *testing.T) {
	if code := callWithAuth(t, "secret123", "Bearer wrong"); code != http.StatusUnauthorized {
		t.Fatalf("코드 = %d, 기대 401", code)
	}
}

// 브라우저 요청은 Caddy가 Basic Auth로 이미 막았습니다. 여기까지 왔다는
// 것은 통과했다는 뜻이라 다시 보지 않습니다.
func TestBearer가없으면통과(t *testing.T) {
	if code := callWithAuth(t, "secret123", ""); code != http.StatusOK {
		t.Fatalf("코드 = %d, 기대 200", code)
	}
	if code := callWithAuth(t, "secret123", "Basic dXNlcjpwYXNz"); code != http.StatusOK {
		t.Fatalf("Basic 인증 헤더는 그대로 흘려보내야 합니다: %d", code)
	}
}

// 토큰이 설정되지 않았는데 Bearer를 통과시키면, 헤더 하나로 인증을
// 건너뛰게 됩니다.
func TestToken이없으면Bearer를거부(t *testing.T) {
	if code := callWithAuth(t, "", "Bearer anything"); code != http.StatusUnauthorized {
		t.Fatalf("코드 = %d, 기대 401", code)
	}
}

// 앞뒤 공백 때문에 실패하면 원인을 찾기 어렵습니다.
func TestBearer앞뒤공백은무시(t *testing.T) {
	if code := callWithAuth(t, "secret123", "Bearer  secret123  "); code != http.StatusOK {
		t.Fatalf("코드 = %d, 기대 200", code)
	}
}

func TestBearer접두사는대소문자를가린다(t *testing.T) {
	// RFC상 scheme은 대소문자를 가리지 않지만, 실수로 다른 스킴을
	// 통과시키지 않도록 정확히 "Bearer "만 봅니다. 다른 표기는 토큰이
	// 없는 것으로 보고 Caddy 인증에 맡깁니다.
	if code := callWithAuth(t, "secret123", "bearer secret123"); code != http.StatusOK {
		t.Fatalf("코드 = %d, 기대 200", code)
	}
}
