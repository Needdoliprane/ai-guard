# Conversation fournisseur sur stdio

Le 25 novembre 2026, sur le candidat `05da0686`. Cette étude
documentaire prépare les adaptateurs Rust de G04/G07 et leurs recettes.
Les preuves batch et TTY conservent le périmètre de leurs gels.
[Contrat de session](../../SESSION_CONTROL.fr.md) · [Preuves fournisseur](../../PROVIDER_AUTH.md#preuves-réelles-historiques-et-limites-actuelles)

## Acquitter une consigne exacte

**Codex app-server.** Sur le même processus stdio, initialiser le
protocole et créer un thread. Utiliser un appel `turn/start` par
consigne et garder un seul tour en vol. Lier l’opération et
l’empreinte du texte envoyé à l’identifiant RPC et au thread attendu.
La réponse typée du même RPC fournit l’identifiant du tour et
acquitte l’entrée côté client. La notification `turn/completed`
corrélée restitue ensuite l’issue du tour. L’idempotence de la
livraison reste à la charge du contrôle.
[Protocole OpenAI](https://learn.chatgpt.com/docs/app-server#start-a-turn) · [Types de la version 0.170.0](https://raw.githubusercontent.com/openai/codex/rust-v0.170.0/codex-rs/app-server-protocol/src/protocol/v2/turn.rs)

**Claude Code.** Anthropic documente les entrées successives sur une
session streaming et l’écho de l’entrée avec `--replay-user-messages`
en JSONL. L’aide archivée de Claude Code 2.1.140 expose ce mode.
Pour reconnaître l’écho comme acquittement, vérifier sur la version
retenue la session, l’incarnation du canal, l’UUID de la consigne et
le contenu exact. Attendre le résultat du premier tour avant
d’envoyer le suivant. Tant que la corrélation exacte reste
indéterminée, l’interface peut attester une écriture complète et
conserve un acquittement inconnu. Le résultat du tour garde son
propre état.
[CLI Anthropic](https://code.claude.com/docs/en/cli-reference#cli-flags) · [Entrée streaming](https://code.claude.com/docs/en/agent-sdk/streaming-vs-single-mode) · [Type de l’écho](https://code.claude.com/docs/en/agent-sdk/typescript#sdkusermessagereplay) · [Aide archivée](../2026-10-30/provider-auth/software/worker-image-probe.log)

Le contrôle conserve séparément l’enregistrement durable de la
consigne, l’écriture entière, l’acquittement fournisseur, le résultat
du tour et l’effet observé. Après une émission possible avec reçu
manquant, conserver une livraison inconnue et réconcilier l’opération
avant toute autre émission. Le contrôle revalide le binding et
l’autorisation de la consigne au point d’effet. STOP ferme d’abord le
droit d’émettre et l’arrêt du worker suit la garde du broker. La
réponse à une demande d’interruption peut précéder l’observation de
fin. Le confinement et le contrôle réseau du worker restent requis.
[Interruption Codex](https://learn.chatgpt.com/docs/app-server#interrupt-a-turn) · [Interruption Claude](https://code.claude.com/docs/en/agent-sdk/streaming-vs-single-mode#interrupt-claudes-response)

## Prévol et validation ciblée

- Borner les trames et les délais. Exercer deux tours avec un enfant de
  fixture. Vérifier un reçu valide, un identifiant discordant, une
  provenance divergente, un doublon, une coupure après émission
  possible, la concurrence avec STOP et une panne d’audit.
- Établir la version et l’empreinte du client pour le gel choisi.
  Codex app-server et le batch ont des chemins d’authentification à
  vérifier séparément. Le compte API ou l’abonnement doit être constaté
  dans le contexte isolé. Pour Claude, garder le matériel du profil
  exclusif, car la présence de la clé API influence la sélection du
  credential. Un mode expiré reste refusé. [Sources Codex 0.170.0](https://raw.githubusercontent.com/openai/codex/rust-v0.170.0/codex-rs/app-server/src/lib.rs) · [Modes OpenAI](https://learn.chatgpt.com/docs/app-server#authentication-modes) · [Priorité Anthropic](https://code.claude.com/docs/en/authentication#authentication-precedence)
- Après qualification du prévol et de l’egress, une recette native
  par couple client/authentification. Deux tours textuels dans le
  même processus, sans outils, avec un marqueur synthétique donné au
  premier tour et retrouvé au second. Vérifier séparément la continuité
  de session et l’acquittement exact de chaque consigne. Pour Claude,
  demander explicitement `claude-haiku-5-5`. Si le modèle servi n’est
  pas observable, le relever comme inconnu. Archiver la preuve
  expurgée, la fin du processus et le modèle demandé.
