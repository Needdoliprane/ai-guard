# Diagnostic runtime et finalisation D1 tardive — 17 novembre 2026

[English](README.md) · [preuve structurée](results.json) · [contrat du relevé](../../../RUNTIME_MONOTONIC_OBSERVATION.md)

Le relevé runtime facultatif et borné marque les étapes réelles du processus
Rust, de l’échéance, des captures et du nettoyage. Ce diagnostic local au
processus ne remplace ni l’audit durable ni une observation indépendante du
moteur. Saturation ou déconnexion ne retardent pas STOP et ne changent pas son
échéance ; une collecte incomplète reste signalée.

**12 tests logiciels ciblés PASS**, une invocation chacun, sur le gel
`c755fb525e98c9bfe768cb5f4fd2f13890318dfe99416e60e06939ad342db3b9`.
La sélection contient sept unités relevé/IO, un cas de conservation des erreurs
et quatre cas D1 avec vrai CLI ai-guard, Gateway et SQLite, et moteur Rust de
fixture. Ces quatre cas vérifient finalisation terminale unique, faute Managed
tardive et rétention prudente du quota/de l’instance pour Created/Unknown.
Noms exacts, empreintes des sorties brutes et durées sont dans `results.json`.

Rust 1.99.1 épinglé sur macOS ARM64 construit les trois binaires avec
`cargo test --lib --test runtime_confinement --test audit_run2_d1_late_finalizer --no-run --locked --offline -j1` ;
les profils QA utilisent opt-level 0, debug 0, assertions et contrôles de
débordement actifs, 256 unités de génération et aucun incrémental. Six listes
normal/ignoré sont vérifiées avant les cas exacts. La feuille de test modifiée
passe `rustfmt --check`. Le check toutes cibles appartient à la source précédente
et n’est pas transféré à celle-ci. Clippy toutes cibles locked/offline de cette
révision retourne RC0 avec **528 émissions d’avertissement et 0 erreur**, sans
mode strict. Les 21 émissions supplémentaires sont un helper de test inutilisé,
une closure redondante et 19 emprunts superflus dans la fixture du test physique ;
elles restent suivies. Le gate strict reste ouvert.

L’ancien test de sonde échouait après deux PASS : il lisait `io.stderr` alors
que l’erreur de commande non nulle possédait déjà cette capture. Le test corrigé
lit cette vraie erreur typée et conserve statut, invocation, deux EOF, deux
marqueurs et échéance d’origine. La fixture vide stderr avant son signal de
readiness stdout. Le FAIL historique reste conservé ; cette assertion échouée
n’établissait pas une perte de capture du produit.

Les sorties brutes privées, empreintes des exécutables et reçus restent hors
checkout. L’exécuteur avait d’abord écrit les traces dans le worktree de
développement ; elles ont été copiées octet pour octet dans le dossier privé,
puis vérifiées par l’intégrateur. Aucun cas n’a été rejoué pour corriger ce chemin.

**NOT_RUN :** vrais Docker/Podman/Kubernetes, clients fournisseurs, reprise
froide, scénario complet A1/A2/B1 et capacité 100. Le cas physique ignoré reste
ignoré. Aucun gate de production n’est clos ; constats primaires et ancien FAIL
runtime physique ne sont pas réécrits.
