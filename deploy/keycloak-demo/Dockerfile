# Source and immutable multi-platform digest verified against the official
# Keycloak download page and Quay API on 2026-11-01; see README.md.
FROM quay.io/keycloak/keycloak:26.8.1@sha256:3f3c5f8e03c614774b4bd9173b88391fefa4153bfc010d8012d59ad825377332

# Prepare Quarkus at build time so the runtime filesystem can remain read-only.
# dev-file and a single local cache are deliberate DEMO-only choices.
RUN /opt/keycloak/bin/kc.sh build --db=dev-file --health-enabled=false --metrics-enabled=false
COPY --chown=1000:0 ai-guard-demo-realm.json /opt/keycloak/data/import/ai-guard-demo-realm.json
ENTRYPOINT ["/opt/keycloak/bin/kc.sh"]
