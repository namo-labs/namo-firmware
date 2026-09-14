# 빌드와 실행을 나눕니다. 실행 이미지에는 바이너리만 들어갑니다.
#
# BUILDPLATFORM으로 고정해 빌더는 러너의 네이티브 아키텍처에서 돌고,
# 결과물만 TARGETARCH로 크로스컴파일합니다. QEMU 에뮬레이션을 피해
# arm64 이미지를 빠르게 만들기 위한 것입니다.
FROM --platform=$BUILDPLATFORM golang:1.23-alpine AS build

WORKDIR /src

# 의존성을 먼저 받아, 소스만 바뀔 때는 이 층을 다시 쓰게 합니다.
COPY go.mod go.sum ./
RUN go mod download

COPY . .

ARG TARGETOS
ARG TARGETARCH
# CGO를 끄면 정적 바이너리가 나와 distroless에서도 돕니다.
RUN CGO_ENABLED=0 GOOS=${TARGETOS} GOARCH=${TARGETARCH} \
    go build -trimpath -ldflags="-s -w" -o /out/namo-monitor .

FROM gcr.io/distroless/static-debian12:nonroot

COPY --from=build /out/namo-monitor /namo-monitor

# 이력 파일이 여기 쌓입니다. 볼륨을 붙이지 않으면 재시작 때 사라집니다.
VOLUME ["/data"]
EXPOSE 8080
USER nonroot:nonroot

ENTRYPOINT ["/namo-monitor"]
