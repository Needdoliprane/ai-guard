# Clients fournisseurs réels — 30 octobre 2026

## Dernier état, distinct des gels historiques ci-dessous

**Claude API batch : PASS ciblé** sur l'ancien binaire interactif `7a86a4ff…e1e3ed`, en invocation manuelle. [Rapport final](claude-api-real.md), [résultat exact](result/claude/result.txt), [reçu exact](result/claude/receipt.txt) et [audit réouvert](result/claude/audit-verification.json). Exit 0, deux fichiers créés/exportés dans une destination neuve, original intact, quota libéré, une allocation observée non facturable, audit 26 valide. Modèle demandé par l'ancien alias `haiku` : ce test n'atteste pas Haiku 5.5. Les prochains essais demandent explicitement `claude-haiku-5-5`.

**Claude TUI : FAIL de qualification par instrumentation**, aucun deuxième message ni retry. [Rapport](claude-tui-real.md) : handle perdu après démarrage par utilisation de `TextEncoder` absent de functions V8 ; arrêt opérateur 137, quota libéré, audit 32 valide et aucune réponse Claude prouvée. Le [wrapper corrigé hors ligne](claude-tui-wrapper-preflight.md) ne qualifie pas un client. Les gateways temporaires batch/TUI sont maintenant arrêtés ; données et preuves restent conservées.

**Codex TUI : toujours non qualifié à deux tours.** Le [diagnostic des outils locaux hors ligne](tui-offline-tools.md), sans clé/réseau/modèle, a observé deux sorties de commandes locales et un retour au composeur. Il conserve son premier défaut de préparation de HOME et un `/exit` non qualifié (timeout). Ne pas le compter comme deux tours modèle réussis.

Ces invocations manuelles ne qualifient pas la [nouvelle résolution dynamique](../provider-auth/README.md), dont le bootstrap/image et les quatre authentifications réelles restent à exercer. Les mentions « Claude versions/aides seulement » des paragraphes suivants décrivent leur gel initial ; le nouveau PASS ci-dessus les complète.

Tests ciblés exécutés par **Luna**, sans campagne E2E globale. Le [rapport Codex](codex-real.md) conserve le premier FAIL et le second PASS ; l'orchestrateur a recoupé le contenu exporté par `cmp` et les SHA256 de l'original et de l'export.

| Statut | Comportement observé |
|---|---|
| FAIL fonctionnel initial | Codex0.165.0, `workspace-write` : exit0 mais aucun fichier. Deux commandes échouent dans `bwrap`, création de namespace utilisateur refusée par le profil Docker durci. La déclaration fournisseur d'un fichier ne devient pas un effet constaté. |
| PASS | Profil strict : refus durable avant worker et fournisseur. |
| PASS | Nouvelle session Codex0.165.0, modèle demandé `gpt-6-luna`, mode interne `danger-full-access` et `approval_policy=never` explicites dans le conteneur contrôlé. Fichier réel créé, export neuf exact, original inchangé. Aucun changement Docker/caps/seccomp/sysctl. |
| PASS | Worker terminal, cleanup effectué et quota relâché. Relevé d'allocation Docker observé, `billable=false` ; aucune mesure de CPU consommé, de prix ou de facture fournisseur déduite. |
| PASS | Audit668 valide ; [résultat](audit-final.json). |
| PASS limité | Image contenant Codex0.165.0 et Claude2.1.140 : versions et aides réelles sous contraintes Docker ; [préparation Claude](claude-preparation.md). |
| NOT_RUN à ce gel | Authentification/modèle Claude et TUI des clients ; tentatives TUI ultérieures détaillées ci-dessous. Le compte OAuth hôte n'a pas été copié ou extrait. |

Le mode interne Codex retenu est documenté pour un [conteneur contrôlé](https://learn.chatgpt.com/docs/non-interactive-mode) ; l'isolation repose ici sur le worker ai-guard. Le réseau bridge est explicitement sortant sans filtrage applicatif. Ce résultat ne prouve ni médiation des commandes transitives, ni contrôle universel du fournisseur, ni modèle servi indépendamment des déclarations du client.

Source de vérité de l'effet : [fichier exporté](result/result.txt), SHA256 `71b2d747aae3bb4e7edd4038d029a5f273329d97001d3b14b70f2d8a2b883577`. Le README original et exporté gardent `7ef29d760972b1e26a8271178786e44e2a10b11990290eea3af406a674e2cd11`. Ce petit projet est explicitement une fixture de travail ; le client et ses appels fournisseur sont réels.

Seuls rapports, résumés protégés et résultats non secrets sont conservés ici. Preuves complètes privées : `/tmp/ai-guard-codex-real-run.rNPuqB/`, dossier700, valeurs sensibles600. Le scan Rust du rapport et de ces preuves retourne `secret_scan_match_files=0`. Aucun fichier de credentials ni sortie brute fournisseur n'est archivé dans le dépôt. Les clés réelles restent hors projet.

Contrôle utilisé : gel SQLite327, image `qa-20261030-sqlite-327-ovpt-8d29af`. Worker Codex : `sha256:42cfc041ee5daa0128da53da45c580e66aa1c17839b5013f053f7221d7179cab`. L'image combinée `qa-claude-2.1.140-codex-0.165.0-20261030`, `sha256:d6b17dcb85e23a240fa0fef25e14247b117e62c5ce700712d5bef04573d1ae73`, qualifie versions/aides seulement. Le prochain preset de journalisation moteurv2 est un autre lot, pas attribué rétroactivement à ces exécutions.

## Codex TUI — tentatives suivantes, qualification encore ouverte

Sur le binaire interactif privé `7a86a4ff…e1e3ed` et l'image Codex ci-dessus, sans `exec`, faux serveur ou cache OAuth hôte : [première session](tui-first.md), effet du premier tour et export exact une ligne, second message resté dans le composeur, arrêt contrôlé137, audit33 valide/quota0/allocation1 observée non facturable. **FAIL/INCOMPLETE deux tours.** Le [diagnostic hors ligne](tui-keyboard.md) sans clé/réseau/modèle confirme `/status` et `/exit` avec texte, pause500ms puis CR séparé, sans établir la cause du premier échec.

La [nouvelle session](tui-two-turns.md) a créé le premier fichier mais n'a pas fourni de preuve observable de reçu/idle ; aucun second message envoyé. À300s, arrêt automatique137/timed_outtrue et motif d'échéance, cleanup effectué, audit26 valide/quota0/allocation1 observée non facturable. Export exact une ligne, original intact. **FAIL du critère deux tours**, sans défaillance broker établie : une réponse d'outil tronquée a pu contenir le reçu, son contenu n'est plus vérifiable. `output_captured=false` est attendu, sans transcript durable ; il ne prouve pas l'absence de réponse. Aucun nouvel appel modèle avant correction de cette collecte de preuve. Les rapports d'essais et leurs fixtures/résultats ont des scans littéraux privés sans hit ; aucune preuve générale de DLP n'en est déduite.
