# Diagnostic clavier TUI Codex — 2026-10-30

## Résultat borné, sans modèle

Une seule session du vrai CLI `codex-cli 0.165.0`, sans prompt positionnel, sans clé/API envfile, sans `env_key`, sans auth cache hôte et avec `--network none`. Fournisseur personnalisé `ai_guard_offline`, `requires_openai_auth=false`, `wire_api=responses`, base URL `http://127.0.0.1:9`. Aucun faux serveur n’a été lancé. Aucun texte de prompt modèle n’a été envoyé; seuls slash-commandes locales officielles ont été utilisées. Le panneau `/status` affichait `Model: GPT-6-Luna`, `Model provider: ai_guard_offline`, `Permissions: Full Access`, `Token usage: 0 total (0 input + 0 output)`.

### Gestes observés

1. Après retour au composeur idle, `write_stdin` a envoyé uniquement le texte `/status`, sans terminator.
2. Pause distincte de 500 ms.
3. Un appel `write_stdin` séparé a envoyé CR `0x0D`. Le TUI a ouvert le panneau `/status` local, affiché les détails, puis est revenu au composeur. Aucun prompt modèle ni requête provider n’a été saisi.
4. L’avertissement visible `1 warning` a été ouvert via la slash-commande locale `/warnings`, saisie sans terminator, pause distincte de 500 ms, puis CR `0x0D` séparé. Texte exact : “Running without the shared background server: command-line configuration overrides (-c, --enable, --disable, or --search) requires embedded mode.” Fermeture par ESC, retour composeur. Cela signale le mode sans serveur partagé; le panneau `/status` affichait malgré tout `ai_guard_offline` sélectionné. Aucune conclusion d’auth/provider n’en découle.
5. `/exit` a été saisi seul, pause distincte de 500 ms, puis CR `0x0D` séparé. L’écran a montré “Shutting down…”, restauré le terminal et quitté avec code 0.

Les octets de saisie sont donc distincts : texte slash-command, délai, puis CR. Lors de la précédente tentative réelle, le CR avait été inclus dans le même envoi que le texte du second prompt; un LF `0x0A` séparé a ensuite été envoyé après attente, mais sans transition claire. Cette comparaison rend plausible une différence d’encadrement/temporisation du clavier; elle ne démontre ni un défaut général du broker ni une règle universelle LF→CR. Aucun changement produit n’est justifié par ce diagnostic seul.

## État final privé

Gateway Rust privé local, SQLite neuf et credentials runner/viewer privés sous `/tmp/ai-guard-codex-tui-keyboard.JKMQZS`; écoute loopback seulement, arrêtée après preuve. Image `agentsandbox-worker:qa-codex-0.165.0-20261030`, ID `sha256:42cfc041ee5daa0128da53da45c580e66aa1c17839b5013f053f7221d7179cab`.

- Session `07e52d78-50db-49a2-99ff-ef406cb0cd3f`, worker `9e4907e2-6991-45ce-a131-e09b7f3eca87`, tentative `fa27e0b3-0688-4020-bfcf-80f4ec8eac34`.
- **PASS** worker terminal, quota non retenu, code de sortie 0, timeout false, aucune modification, cleanup removed, aucune erreur de cleanup/post-traitement.
- **PASS** audit valide : 25 événements, tête `2107ed38283fd2f3bc08cad0bff4bfda9357feedaa18d65f7093e586c3563e30`.
- **PASS** une allocation correspondant exactement à la tentative, `billable=false`, méthode 1, runtime docker, provenance observed.
- Le projet scratch n’a pas été exporté car aucun fichier n’a changé.

La session ne qualifie pas l’auth API, la disponibilité de Luna, l’envoi d’un tour modèle ou le TUI Codex avec credentials. La valeur de clé utilisateur ni son fichier n’ont été lus par cette préparation. Aucun enregistrement transcript brut n’a été conservé. Aucun moteur/provider/browser/E2E hors de cette session et de ses vérifications terminales/audit/usage.
