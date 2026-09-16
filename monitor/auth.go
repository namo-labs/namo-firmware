package main

import (
	"crypto/subtle"
	"net/http"
	"strings"
)

// withAuth는 Bearer 토큰이 붙은 요청을 검증합니다.
//
// 인증이 두 갈래인 이유는 쓰는 쪽이 다르기 때문입니다.
//
//   - **브라우저**는 Caddy가 Basic Auth로 막습니다. 사람이 한 번
//     입력해두면 계속 쓰므로 그쪽이 편합니다. 여기까지 온 요청은 이미
//     통과한 것이라 다시 보지 않습니다.
//   - **앱**은 Bearer 토큰으로 옵니다. Basic Auth는 자격증명을 앱에
//     심어야 하고 계정 단위라, 기기별로 끊거나 돌리기 어렵습니다.
//
// 토큰이 설정되지 않았으면 Bearer 요청을 **거부합니다**. 검증할 수단이
// 없는데 통과시키면 헤더 하나로 인증을 건너뛰게 됩니다.
func withAuth(token string, next http.Handler) http.Handler {
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		const prefix = "Bearer "
		auth := r.Header.Get("Authorization")

		if strings.HasPrefix(auth, prefix) {
			got := strings.TrimSpace(strings.TrimPrefix(auth, prefix))
			// 길이가 다르면 ConstantTimeCompare가 0을 돌려주므로 따로
			// 길이를 보지 않아도 됩니다.
			if token == "" || subtle.ConstantTimeCompare([]byte(got), []byte(token)) != 1 {
				w.Header().Set("WWW-Authenticate", `Bearer realm="namo"`)
				writeJSON(w, http.StatusUnauthorized, map[string]any{
					"error": "토큰이 올바르지 않습니다",
				})
				return
			}
		}

		next.ServeHTTP(w, r)
	})
}
