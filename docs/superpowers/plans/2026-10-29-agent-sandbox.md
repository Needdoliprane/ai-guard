# Agent Sandbox Implementation Plan

> **Plan historique du 29 octobre 2026, conservé pour traçabilité.** Les cases cochées et le découpage MVP/V2 ci-dessous décrivent le premier lot logiciel dans son environnement d'origine ; ils ne qualifient pas le checkout courant et ne définissent plus le périmètre. Le mandat actuel est le produit complet de `AGENTS.md` et `PROMPT_PRODUCTION.md`, dans l'architecture validée. Suivre `docs/engineering/IMPLEMENTATION_PLAN.md`, `docs/engineering/CURRENT_STATE.md`, `docs/engineering/VALIDATION.md` et les gates G01–G12. Aucune obligation ne peut être reportée sur la base de ce plan historique.

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [x]`) syntax for tracking.

**Goal:** Livrer une sandbox Docker/Podman avec supervision conteneurisée, audit et broker Rust, front TypeScript et garanties documentées.

**Architecture:** Gateway/collecteur et dashboard séparés. CLI de confiance pilotant le runtime sans socket dans les conteneurs. Projet copié dans un volume de session, commandes isolées et sorties exportables.

**Tech Stack:** Rust 1.99.1, axum 0.8, tokio 1, rusqlite, reqwest/rustls, clap ; TypeScript/Vite.

**Spec:** `docs/design.md` et cahier des charges joint fourni par l'utilisateur.

## Global Constraints

- Tout le code applicatif hors front est en Rust. Front TypeScript.
- Docker et Podman ; aucun moteur présent dans cet environnement au départ.
- IHM dans un conteneur distinct des agents, aucun socket privilégié dans les services.
- Absence de moteur, API ou métrique = état inconnu/non validé explicite.
- Masquage des secrets connus/reconnus avant persistance ; aucune commande métier sur l'hôte. Les limites du masquage sont documentées.
- Pas de transmission externe de contenu à Jev sans activation explicite.

## Review Focus

- Injection de shell/arguments et programmes qui exécutent des sous-processus : pas de shell hôte, distinction contrôle de lancement/couverture transitive.
- Chemins d'archive, liens symboliques et secrets dans une copie de projet : validation avant import et export.
- Audit indisponible ou plein : le broker ne lance rien sans trace durable.
- Authentification, XSS, SSE et redaction : sorties traitées comme texte non fiable, tokens hors URL.
- Runtime absent ou refusé : pas de fallback d'exécution locale ; statut de test honnête.

## Tâche 1 — Noyau de sécurité et audit (parent)

Fichiers : `Cargo.toml`, `src/model.rs`, `src/audit.rs`, `src/redaction.rs`, `src/policy.rs`, `src/jev.rs`, `src/api.rs`, `src/lib.rs`, `src/main.rs`, tests.

- [x] Créer tests de refus, persistance, intégrité, idempotence, masquage et authentification avant implémentation.
- [x] Implémenter les modèles sérialisés et le stockage transactionnel SQLite WAL.
- [x] Implémenter gateway, dashboard proxy lecture seule et SSE à curseur.
- [x] Implémenter politiques déterministes et client Jev sur contrat officiel.
- [x] Exécuter `cargo test` et valider les chemins d'erreur.

## Tâche 2 — Front (agent indépendant)

Fichiers : `front/`, `docs/ui.md`. Consomme API décrite dans design.md et contrat front communiqué.

- [x] Créer interface TypeScript responsive clair/sombre, écran de connexion par token.
- [x] Vues overview/agents/actions/security/files/sandboxes et détail d'événement, filtre/export.
- [x] Afficher provenance et couverture ; aucun faux agent ou métrique.
- [x] Compiler et vérifier DOM/API avec tests pertinents ; capture de contrôle locale si possible.

## Tâche 3 — Runtime, CLI et packaging (agent indépendant)

Fichiers : `src/runtime.rs`, `src/runner.rs`, `src/workspace.rs`, `deploy/`, `images/`, scripts Rust ou commandes Cargo, `docs/operations.md`.

- [x] Tests de construction de commandes sans shell, exclusion de secrets et chemins malveillants.
- [x] Implémenter création de volumes, import/export sûrs, exécution et collecte stdout/stderr bornées.
- [x] Implémenter lifecycle audit requested/decision/submitted/started/completed et diff avant/après.
- [x] Images/Compose Docker et Podman avec service dashboard conteneurisé.
- [x] Procédure reproductible et smoke test ignoré explicitement si moteur absent.

## Tâche 4 — Recherche et dossier (agent puis parent)

- [x] Sources officielles agents et Jev ; matrice capacités documentées/intégration livrée.
- [x] Comparaison stacks, menaces/contrôles/vérifications, modèle JSON, API, tests/acceptation, MVP/V2.
- [x] Confronter chaque section du prompt à la livraison et marquer les limites.

## Tâche 5 — Intégration et revue

- [x] Compiler Rust, tester le noyau et build TypeScript.
- [x] Test de parcours API réel, auth et persistance ; vérifier le front contre cette API.
- [x] Revue indépendante ciblée sur les frontières de confiance, corriger les défauts matériels.
- [x] Consigner résultats et limites dans `docs/validation.md`.
- [x] Emballer le projet complet, enregistrer l'archive et livrer les commandes de lancement.

## Bilan de réalisation

Les cases cochées désignent le travail logiciel livré. Elles ne signifient pas que les essais avec moteurs ou comptes fournisseurs ont été exécutés. Voir `docs/validation.md` et `docs/requirements-coverage.md` pour les preuves, limites et fonctionnalités V2.
