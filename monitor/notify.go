package main

import (
	"context"
	"fmt"
	"log"
	"net/http"
	"net/url"
	"strings"
	"sync"
	"time"
)

// Notifier는 사람이 손을 써야 하는 일이 생겼을 때 텔레그램으로 알립니다.
//
// **알림은 적을수록 좋습니다.** 같은 말을 반복하면 읽지 않게 되고, 정작
// 중요한 알림도 함께 묻힙니다. 그래서 두 가지를 지킵니다.
//
//   - 상태가 **바뀌는 순간**에만 보냅니다. 물통이 빈 동안 계속 보내지
//     않고, 비는 순간 한 번만 보냅니다.
//   - 풀렸을 때도 한 번 보냅니다. 조치한 것이 먹혔는지 알아야 합니다.
type Notifier struct {
	token  string
	chatID string
	client *http.Client

	mu sync.Mutex
	// 지금 켜져 있는 경보들. 같은 경보를 다시 보내지 않기 위한 것입니다.
	active map[string]time.Time

	// 시험에서 실제로 보내지 않고 내용만 보기 위한 자리입니다.
	hook func(text string)
}

func NewNotifier(token, chatID string) *Notifier {
	if token == "" || chatID == "" {
		return nil // 설정이 없으면 알리지 않습니다.
	}
	return &Notifier{
		token:  token,
		chatID: chatID,
		client: &http.Client{Timeout: 10 * time.Second},
		active: make(map[string]time.Time),
	}
}

// Raise는 경보를 올립니다. 이미 켜져 있으면 아무것도 하지 않습니다.
func (n *Notifier) Raise(key, text string) {
	if n == nil {
		return
	}
	n.mu.Lock()
	_, on := n.active[key]
	if on {
		n.mu.Unlock()
		return
	}
	n.active[key] = time.Now()
	n.mu.Unlock()

	n.send(text)
}

// Clear는 경보를 내립니다. 켜져 있던 경우에만 복구를 알립니다.
func (n *Notifier) Clear(key, text string) {
	if n == nil {
		return
	}
	n.mu.Lock()
	since, on := n.active[key]
	if !on {
		n.mu.Unlock()
		return
	}
	delete(n.active, key)
	n.mu.Unlock()

	if text != "" {
		n.send(text + "\n\n" + fmt.Sprintf("(%s 동안 이어졌습니다)", humanDur(time.Since(since))))
	}
}

// Notify는 경보와 무관하게 한 번 알립니다.
func (n *Notifier) Notify(text string) {
	if n == nil {
		return
	}
	n.send(text)
}

func (n *Notifier) send(text string) {
	if n.hook != nil {
		n.hook(text)
		return
	}

	endpoint := "https://api.telegram.org/bot" + n.token + "/sendMessage"
	form := url.Values{
		"chat_id":    {n.chatID},
		"text":       {text},
		"parse_mode": {"HTML"},
		// 링크 미리보기가 붙으면 알림이 길어집니다.
		"disable_web_page_preview": {"true"},
	}

	ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
	defer cancel()

	req, err := http.NewRequestWithContext(ctx, http.MethodPost, endpoint,
		strings.NewReader(form.Encode()))
	if err != nil {
		log.Printf("알림을 만들지 못했습니다: %v", err)
		return
	}
	req.Header.Set("Content-Type", "application/x-www-form-urlencoded")

	resp, err := n.client.Do(req)
	if err != nil {
		log.Printf("알림 전송 실패: %v", err)
		return
	}
	defer resp.Body.Close()
	if resp.StatusCode != http.StatusOK {
		log.Printf("알림이 거부됐습니다: HTTP %d", resp.StatusCode)
		return
	}
	log.Printf("알림 보냄: %s", firstLine(text))
}

func firstLine(s string) string {
	if i := strings.IndexByte(s, '\n'); i >= 0 {
		return s[:i]
	}
	return s
}

func humanDur(d time.Duration) string {
	s := int(d.Seconds())
	switch {
	case s < 60:
		return fmt.Sprintf("%d초", s)
	case s < 3600:
		return fmt.Sprintf("%d분", s/60)
	case s < 86400:
		return fmt.Sprintf("%d시간 %d분", s/3600, (s%3600)/60)
	default:
		return fmt.Sprintf("%d일 %d시간", s/86400, (s%86400)/3600)
	}
}
