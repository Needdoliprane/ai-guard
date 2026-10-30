# Correctifs après l'audit indépendant — gel235

L'audit figé conserve19 `needs_validation`,0 confirmé. Ce dossier contient les preuves de développement de deux correctifs sur le checkout ultérieur ; ce n'est pas une nouvelle confirmation formelle du workflow Cloudflare.

| Contrôle | Résultat |
|---|---|
| fmt, Clippy all-targets, suite, release | PASS/exit0 ;235 tests exécutés,0FAIL,3ignorés ; fichiers `ai-guard-followup-{fmt,clippy,tests,release}.log` |
| Jev | RED CLI/init puis RED bibliothèque, correctif et revue ; séparation de tous les credentials de contrôle avant publication/client, assemblages local et multi-tenant, clés normalisées, politique conservée |
| Capture | RED256KiB/périodicité/secrets imbriqués/chevauchants, correction union d'octets KMP avant décodage, conservation des JSONL complets ; rapports capture et tests ciblés |
| Erreur CLI64KiB | Cause typée, Display/Debug opaques, compteurs/troncature conservés, fusion des secrets du caller et des marqueurs privés avant masquage ; contrôles échappés avant raisons durables |
| Bootstrap Podman | Fixture Rust : volume create125 écho nom/nonce avant bindingSome, contexte utile conservé, valeurs absentes du rendu ; ce test ne qualifie pas Podman réel |
| Docker actuel | Confinement1PASS/4,14s, terminal refusé1PASS/3,13s ; mêmes limites nommées que les tests, pas de panne disque ou durée après SIGKILL |
| Smoke après déploiement |1PASS/22,24s ; UID/copie/export/refus, allocation unique après reprise, puis secret synthétique4097octets×64 sur stdout ET stderr, indicateurs de troncature et contenu utile conservés, absence du préfixe en audit |
| Navigateur déployé |10 contrôles PASS en mode partagé, aucune nouvelle CSS ; vues réelles, identité, SSE/reconnexion/export, rôles et bundle |

Le smoke de capture utilise le profil existant explicite `observe + bridge`, nécessaire au chemin de credentials actuel. Le worker exécute uniquement une boucle printf avec une valeur synthétique ; aucun appel fournisseur, compte réel ou accès cloud n'est effectué. Les fixtures de secrets restent dans les tests ; la réponse API correspond à une exécution réelle QA. Ce résultat ne prouve ni prévention d'encodages arbitraires, ni DLP générale, ni isolation réseau multi-client.

Image déployée `agentsandbox-control:qa-20261029-secrets-a64d7b0e`, ID `sha256:4eb348a836fe51ae3325aa5c078a98a505ab0dc87306762f35b1faf567c12a78`. Projet QA et volume existants conservés ; aucune migration de schéma pour ce lot. Audit avant/après redéploiement identique à270 événements. Après nouveaux essais :347 événements, head `5024735d727aa4a4217328d8b9a9070cef3ffe754e0ac819155d35281a258af5`, valide. La configuration active est `/tmp/ai-guard-registry-qa.40IImn/compose-secrets.yaml` ; les configurations et preuves précédentes restent en place.

`SOURCE_SHA256SUMS.txt` identifie99 entrées, vérifiées au gel ; `RELEASE_SHA256SUMS.txt` identifie le binaire hôte. Les rapports indépendants lisent source et résultats fournis, sans exécution cible personnelle. Les images/packages ne sont pas déclarés reproductibles ou signés.

Les candidats PATH, ancrage configuration/SQLite/credentials et ACL, drainage/contention, stockage/logs, ainsi que supervision durable, relais, matrices runtime/cloud, capacité100 et qualification production restent ouverts. [AUDIT_FOLLOWUP.md](../../../AUDIT_FOLLOWUP.md) conserve leur suivi. La référence visuelle Nothin’ est une consigne de gouvernance ; sa déclinaison CSS reste à réaliser et vérifier.
