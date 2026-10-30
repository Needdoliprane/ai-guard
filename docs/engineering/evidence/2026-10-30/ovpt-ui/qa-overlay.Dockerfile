FROM agentsandbox-control:qa-20261030-editorial-fs-6f4d8b02
# QA frontend-only overlay: retain the validated backend, no database migration.
USER root
RUN rm -rf /app/front && mkdir -p /app/front
COPY dist/ /app/front/
USER 1000:1000
