#!/usr/bin/env bash
# Pinned single-node Kafka (KRaft) for the AsyncAPI receive live tests, CI,
# docs, and the recorded evidence run.
#
#   scripts/kafka/broker.sh up [--secure]     start; prints bootstrap address(es)
#   scripts/kafka/broker.sh down              graceful stop (container runs with --rm)
#   scripts/kafka/broker.sh topic <name> [n]  create a topic with n partitions (default 1)
#   scripts/kafka/broker.sh produce <topic> [--codec none|gzip|snappy|lz4|zstd] [--key <k>]
#                                             stdin lines: headers<TAB>payload;
#                                             headers are h1:v1,h2:v2 (at least one;
#                                             the console producer rejects an empty field);
#                                             --key <k> sends every line with record key k
#   scripts/kafka/broker.sh groups            list consumer groups (expected: none)
#   scripts/kafka/broker.sh connections       count established client connections
#
# Environment: ARAZZO_KAFKA_PORT (19092), ARAZZO_KAFKA_SECURE_PORT (19093),
# ARAZZO_KAFKA_STATE_DIR (/tmp/arazzo-kafka), ARAZZO_KAFKA_CONTAINER (arazzo-kafka).
#
# --secure adds a SASL_SSL listener with PEM certificates generated here and
# SCRAM-SHA-256/512 user "arazzo" / "arazzo-secret" (PLAIN disabled). The CA
# is written to "$STATE_DIR/ca.pem" for --kafka-ca-cert / ARAZZO_KAFKA_CA_PEM.
#
# The broker deliberately runs with auto.create.topics.enable=true so the
# tests prove the runtime never asks for a topic that does not exist.
set -euo pipefail

# The one place the pin lives. Docs quote it; CI uses it through this script.
IMAGE="apache/kafka@sha256:852a0d49fc4e656b022e7f23785411802c9a088c78a7c5e39f33969de9c227a7"
IMAGE_TAG_HINT="apache/kafka:4.3.2"

CONTAINER="${ARAZZO_KAFKA_CONTAINER:-arazzo-kafka}"
PORT="${ARAZZO_KAFKA_PORT:-19092}"
SECURE_PORT="${ARAZZO_KAFKA_SECURE_PORT:-19093}"
STATE_DIR="${ARAZZO_KAFKA_STATE_DIR:-/tmp/arazzo-kafka}"
SCRAM_USER="arazzo"
SCRAM_PASSWORD="arazzo-secret"

die() { echo "broker.sh: $*" >&2; exit 1; }

need_docker() {
  command -v docker >/dev/null 2>&1 || die "docker is not installed"
  docker info >/dev/null 2>&1 || die "the Docker daemon is not reachable"
}

in_container() { docker exec -i "$CONTAINER" "$@"; }

generate_pem() {
  command -v openssl >/dev/null 2>&1 || die "openssl is required for --secure"
  local dir="$1"
  openssl req -x509 -newkey rsa:2048 -nodes -keyout "$dir/ca.key" -out "$dir/ca.pem" \
    -days 30 -subj "/CN=arazzo-kafka-test-ca" >/dev/null 2>&1
  openssl req -newkey rsa:2048 -nodes -keyout "$dir/server.key" -out "$dir/server.csr" \
    -subj "/CN=127.0.0.1" >/dev/null 2>&1
  printf 'subjectAltName=IP:127.0.0.1,DNS:localhost\n' > "$dir/san.cnf"
  openssl x509 -req -in "$dir/server.csr" -CA "$dir/ca.pem" -CAkey "$dir/ca.key" \
    -CAcreateserial -out "$dir/server.crt" -days 30 -extfile "$dir/san.cnf" >/dev/null 2>&1
  # Kafka's PEM keystore wants an unencrypted PKCS#8 key followed by the chain.
  openssl pkcs8 -topk8 -nocrypt -in "$dir/server.key" -out "$dir/server.pk8" >/dev/null 2>&1
  cat "$dir/server.pk8" "$dir/server.crt" > "$dir/server.pem"
  chmod 644 "$dir/ca.pem" "$dir/server.pem"
}

write_properties() {
  local secure="$1" file="$STATE_DIR/server.properties"
  {
    echo "process.roles=broker,controller"
    echo "node.id=1"
    echo "controller.quorum.voters=1@localhost:9093"
    echo "controller.listener.names=CONTROLLER"
    # INTERNAL is for the tools run inside the container (and inter-broker);
    # its advertised address must resolve inside the container, unlike the
    # host-facing listeners below.
    echo "inter.broker.listener.name=INTERNAL"
    echo "log.dirs=/tmp/kraft-logs"
    echo "offsets.topic.replication.factor=1"
    echo "transaction.state.log.replication.factor=1"
    echo "transaction.state.log.min.isr=1"
    echo "group.initial.rebalance.delay.ms=0"
    echo "num.partitions=1"
    echo "auto.create.topics.enable=true"
    if [ "$secure" = 1 ]; then
      echo "listeners=INTERNAL://0.0.0.0:9095,PLAINTEXT://0.0.0.0:9092,SASL_SSL://0.0.0.0:9094,CONTROLLER://0.0.0.0:9093"
      echo "advertised.listeners=INTERNAL://localhost:9095,PLAINTEXT://127.0.0.1:${PORT},SASL_SSL://127.0.0.1:${SECURE_PORT}"
      echo "listener.security.protocol.map=CONTROLLER:PLAINTEXT,INTERNAL:PLAINTEXT,PLAINTEXT:PLAINTEXT,SASL_SSL:SASL_SSL"
      echo "sasl.enabled.mechanisms=SCRAM-SHA-256,SCRAM-SHA-512"
      echo "listener.name.sasl_ssl.scram-sha-256.sasl.jaas.config=org.apache.kafka.common.security.scram.ScramLoginModule required;"
      echo "listener.name.sasl_ssl.scram-sha-512.sasl.jaas.config=org.apache.kafka.common.security.scram.ScramLoginModule required;"
      echo "ssl.keystore.type=PEM"
      echo "ssl.keystore.location=/arazzo/server.pem"
      echo "ssl.client.auth=none"
    else
      echo "listeners=INTERNAL://0.0.0.0:9095,PLAINTEXT://0.0.0.0:9092,CONTROLLER://0.0.0.0:9093"
      echo "advertised.listeners=INTERNAL://localhost:9095,PLAINTEXT://127.0.0.1:${PORT}"
      echo "listener.security.protocol.map=CONTROLLER:PLAINTEXT,INTERNAL:PLAINTEXT,PLAINTEXT:PLAINTEXT"
    fi
  } > "$file"
}

MARKER=".arazzo-kafka-harness"

# Refuse to treat anything but a directory this script created as harness
# state: `down` deletes it.
state_dir_is_ours() {
  case "$STATE_DIR" in
    ""|/|/tmp|/var|/var/tmp|/private/tmp|"$HOME"|"$HOME"/) return 1 ;;
  esac
  [ -f "$STATE_DIR/$MARKER" ]
}

cmd_up() {
  local secure=0
  [ "${1:-}" = "--secure" ] && secure=1
  need_docker
  if docker ps --format '{{.Names}}' | grep -qx "$CONTAINER"; then
    die "container $CONTAINER is already running (scripts/kafka/broker.sh down)"
  fi
  case "$STATE_DIR" in
    ""|/|/tmp|/var|/var/tmp|/private/tmp|"$HOME"|"$HOME"/) die "ARAZZO_KAFKA_STATE_DIR=$STATE_DIR is not a dedicated directory" ;;
  esac
  if [ -e "$STATE_DIR" ] && [ ! -f "$STATE_DIR/$MARKER" ]; then
    die "ARAZZO_KAFKA_STATE_DIR=$STATE_DIR exists but was not created by this script"
  fi
  mkdir -p "$STATE_DIR"
  : > "$STATE_DIR/$MARKER"
  write_properties "$secure"
  local format_args=""
  local publish=(-p "127.0.0.1:${PORT}:9092")
  if [ "$secure" = 1 ]; then
    generate_pem "$STATE_DIR"
    publish+=(-p "127.0.0.1:${SECURE_PORT}:9094")
    format_args="--add-scram SCRAM-SHA-256=[name=${SCRAM_USER},password=${SCRAM_PASSWORD}] --add-scram SCRAM-SHA-512=[name=${SCRAM_USER},password=${SCRAM_PASSWORD}]"
  fi
  # The image's default entrypoint cannot add SCRAM users at format time, so
  # both modes format and start explicitly for one code path.
  docker run -d --rm --name "$CONTAINER" "${publish[@]}" \
    -v "$STATE_DIR:/arazzo:ro" --entrypoint /bin/bash "$IMAGE" -c \
    "set -e; ID=\$(/opt/kafka/bin/kafka-storage.sh random-uuid); \
     /opt/kafka/bin/kafka-storage.sh format -t \$ID -c /arazzo/server.properties ${format_args} >/dev/null && \
     exec /opt/kafka/bin/kafka-server-start.sh /arazzo/server.properties" >/dev/null
  local i
  for i in $(seq 1 60); do
    if docker logs "$CONTAINER" 2>&1 | grep -q 'Kafka Server started'; then
      echo "ARAZZO_KAFKA_BOOTSTRAP=127.0.0.1:${PORT}"
      if [ "$secure" = 1 ]; then
        echo "ARAZZO_KAFKA_SECURE_BOOTSTRAP=127.0.0.1:${SECURE_PORT}"
        echo "ARAZZO_KAFKA_CA_PEM=${STATE_DIR}/ca.pem"
      fi
      echo "image ${IMAGE} (${IMAGE_TAG_HINT})" >&2
      return 0
    fi
    if ! docker ps --format '{{.Names}}' | grep -qx "$CONTAINER"; then
      die "container exited during startup"
    fi
    sleep 1
  done
  docker logs "$CONTAINER" >&2 || true
  die "broker did not report 'Kafka Server started' within 60s"
}

cmd_down() {
  need_docker
  if docker ps --format '{{.Names}}' | grep -qx "$CONTAINER"; then
    docker stop "$CONTAINER" >/dev/null
  fi
  if state_dir_is_ours; then
    rm -rf -- "$STATE_DIR"
  elif [ -e "$STATE_DIR" ]; then
    echo "broker.sh: leaving $STATE_DIR alone (not created by this script)" >&2
  fi
}

cmd_topic() {
  local name="${1:?topic name}" partitions="${2:-1}"
  in_container /opt/kafka/bin/kafka-topics.sh --bootstrap-server localhost:9095 \
    --create --topic "$name" --partitions "$partitions" --replication-factor 1 >/dev/null
  echo "$name"
}

cmd_produce() {
  local topic="${1:?topic}"; shift
  local codec="none" key=""
  while [ $# -gt 0 ]; do
    case "$1" in
      --codec) codec="${2:?--codec needs a value}"; shift 2 ;;
      --key) key="${2:?--key needs a value}"; shift 2 ;;
      *) die "produce: unknown argument $1" ;;
    esac
  done
  local props=(--property parse.headers=true)
  if [ -n "$key" ]; then
    # The console producer only takes keys from the input line
    # (headers<TAB>key<TAB>value with parse.key), so insert the fixed key
    # after the headers of every line. The key travels through ENVIRON, not
    # `awk -v`, because -v interprets backslash escapes and would corrupt a
    # key such as order\next.
    props+=(--property parse.key=true)
    ARAZZO_KAFKA_RECORD_KEY="$key" awk 'BEGIN { FS = "\t"; OFS = "\t"; k = ENVIRON["ARAZZO_KAFKA_RECORD_KEY"] } { h = $1; sub(/^[^\t]*\t/, ""); print h, k, $0 }' \
      | in_container /opt/kafka/bin/kafka-console-producer.sh --bootstrap-server localhost:9095 \
          --topic "$topic" --compression-codec "$codec" "${props[@]}" >/dev/null
    return
  fi
  in_container /opt/kafka/bin/kafka-console-producer.sh --bootstrap-server localhost:9095 \
    --topic "$topic" --compression-codec "$codec" "${props[@]}" >/dev/null
}

cmd_groups() {
  in_container /opt/kafka/bin/kafka-consumer-groups.sh --bootstrap-server localhost:9095 --list
}

cmd_connections() {
  # Established TCP connections to the host-facing client listeners
  # (9092 = 0x2384, 9094 = 0x2386) as seen inside the container; the
  # INTERNAL listener (9095) carries the broker's own and the tools'
  # connections and is excluded. First line: count.
  in_container sh -c 'cat /proc/net/tcp /proc/net/tcp6 2>/dev/null' | awk '
    NR > 0 && $4 == "01" {
      split($2, local, ":"); port = toupper(local[length(local)]);
      if (port == "2384" || port == "2386") { n++; peers = peers " " $3 }
    }
    END { print n + 0; if (n > 0) print peers }'
}

case "${1:-}" in
  up) shift; cmd_up "$@" ;;
  down) cmd_down ;;
  topic) shift; cmd_topic "$@" ;;
  produce) shift; cmd_produce "$@" ;;
  groups) cmd_groups ;;
  connections) cmd_connections ;;
  *) sed -n '2,20p' "$0" >&2; exit 2 ;;
esac
