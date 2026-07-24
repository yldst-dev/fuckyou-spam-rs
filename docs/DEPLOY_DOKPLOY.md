# Dokploy Compose 배포

## 배포 설정

Dokploy에서 Compose 프로젝트를 만들고 저장소의 `compose.yaml`을 사용합니다. 이 봇은 Telegram long polling 방식이므로 도메인과 공개 포트가 필요하지 않습니다. 복제본은 반드시 1개로 유지합니다.

`.env.example`을 기준으로 `.env`를 만들고 `TELEGRAM_BOT_TOKEN`, `CEREBRAS_API_KEY`, 관리자 및 허용 채팅 값을 설정합니다. 비밀값은 저장소에 커밋하지 않고 Dokploy 접근 권한도 최소화합니다.

Dokploy는 Environment 값을 Compose 파일 옆의 `.env`로 저장하고 `env_file`이 이를 컨테이너에 주입합니다. `COMPOSE_PROJECT_NAME`, Dokploy 애플리케이션 이름 또는 `bot-data` 볼륨 이름을 변경하면 기존 볼륨과의 연결이 달라질 수 있습니다.

서버가 사용하는 공인 IPv4와 IPv6를 쉼표로 구분해 `WEBPAGE_BLOCKED_IPS`에 입력합니다.

```text
WEBPAGE_BLOCKED_IPS=203.0.113.10,2001:db8::10
```

각 값은 CIDR이 아닌 단일 IP 주소여야 합니다. 잘못된 값이 있으면 안전하게 시작을 중단합니다. 이 목록은 최초 URL과 모든 리다이렉트의 DNS 결과 전체에 적용되어 서버 공인 IP를 통한 hairpin 접근을 차단합니다.

웹페이지 가져오기는 HTTP와 HTTPS만 허용하며 목적지 포트는 80 또는 443으로 제한됩니다. `WEBPAGE_FETCH_TIMEOUT`은 개별 요청이 아니라 DNS 조회, 전체 리다이렉트 체인, 응답 본문 읽기를 합친 단일 deadline입니다. 로그에는 scheme, host, 명시 포트만 남고 path, query, fragment는 남지 않습니다.

## 저장소와 런타임

SQLite 데이터는 `bot-data` named volume의 `/app/data`에 저장됩니다. 컨테이너를 다시 만들 때 이 volume을 삭제하지 않습니다. Dokploy 호스트의 volume 백업을 별도로 구성하고 복구 절차를 주기적으로 시험합니다.

Compose는 읽기 전용 루트 파일시스템, 모든 capability 제거, `no-new-privileges`, PID·CPU·메모리 제한, 실행 불가 임시 디렉터리, 로그 회전을 적용합니다.

애플리케이션 내부 자동 업데이트와 예약 재시작 기능은 포함하지 않습니다. 업데이트는 Git push 후 Dokploy 재배포로 수행하며 런타임 재시작은 Docker의 `restart: unless-stopped` 정책과 Dokploy가 담당합니다.

보안 마이그레이션 3은 화이트리스트를 유지하면서 기존 스팸 캐시와 원문 저장 테이블을 한 번 삭제합니다. 최초 배포 직전에 볼륨을 백업하고, 배포 후 캐시가 새 채팅별 해시 구조로 다시 축적되는지 확인합니다.

## 이미지 공급망

빌더와 런타임 기반 이미지는 2026-07-24에 확인한 Docker Hub 멀티아키텍처 manifest digest로 고정되어 있습니다. 기반 이미지 보안 업데이트를 반영할 때는 태그의 새 digest와 변경 내역을 검토한 뒤 Dockerfile의 태그와 digest를 함께 갱신하고 amd64와 arm64 빌드를 검증합니다.

Cargo는 `Cargo.lock`과 `--locked`를 사용합니다. BuildKit `target` 캐시는 사용하지 않으며 registry 캐시는 `fuckyou-spam-rust-cargo-registry-v1` ID로 프로젝트 전용 분리합니다. 같은 Docker builder를 신뢰할 수 없는 프로젝트와 공유하지 않는 것이 가장 안전합니다.

## 검증

배포 전 다음 명령으로 최종 Compose 구성을 확인합니다.

```sh
docker compose --env-file .env config
docker compose build --pull
docker compose up -d
docker compose ps
docker compose logs --tail=100 spam-bot
```

healthcheck가 `healthy`가 되고 Telegram polling이 시작되는지 확인합니다. 재배포 후에도 기존 SQLite 파일과 캐시 기록이 유지되는지 확인합니다. Docker는 `unhealthy` 상태만으로 재시작하지 않지만 애플리케이션 내부 heartbeat 모니터가 처리기 정지를 감지해 프로세스를 종료하고 Compose 재시작 정책이 복구합니다. 비상 재시작 cooldown 파일도 `bot-data` 볼륨에 유지됩니다.

## 참고

- [Dokploy Docker Compose](https://docs.dokploy.com/docs/core/docker-compose)
- [Dokploy Volume Backups](https://docs.dokploy.com/docs/core/volume-backups)
- [SQLite WAL](https://www.sqlite.org/wal.html)
