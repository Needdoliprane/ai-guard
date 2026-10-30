# Authentifier avant de traiter le contenu — 30 octobre 2026

**PASS au périmètre corrigé**, sans qualification générale DoS ou HTTP. Les routes gateway protégées résolvent désormais l'identité et le rôle avant extraction des paramètres et du JSON. Un viewer ne peut imposer la lecture d'un corps de mutation. Tenant et base viennent toujours du credential authentifié.

| Étape | Résultat et preuve |
|---|---|
| RED sur source inchangée | [Log original](logs/red-final.log) : 4 groupes FAIL, 2 PASS. Corps non autorisés lus, erreurs JSON/query avant auth, double Authorization accepté ; un vrai client TCP n'envoyant que les headers attendait encore sa réponse après 2 s. |
| GREEN | [6 groupes PASS](logs/green-new-target.log), zéro ignoré : quatre familles POST et deux alias, corps non consommés sans droit, erreurs runner conservées, identité A/B, headers ambigus/oversize refusés, premier statut HTTP401 obtenu sans corps envoyé. |
| Compatibilité | [45 PASS dans huit suites](logs/green-regressions.log), zéro ignoré : API/tenants, HTTP avec crash/restart du gateway, workers/dashboard fixe et partagé, décisions idempotentes, séparation Jev et contexte fournisseur. Ces tests ne lancent aucun moteur ni fournisseur réel. |
| Analyse statique | [Fmt](logs/fmt-final.log) et [Clippy all-targets strict](logs/clippy-final.log) PASS. |
| Revue indépendante | [Acceptée](review.md), aucun nouveau P0/P1/P2 identifié dans le delta ; limites conservées. |
| Artefact | [Build release unique PASS](artifacts.md), 29,85 s, manifestes Cargo/sources identiques avant/après. Nouveau binaire `b7565088f8e10c05d24b5cc3b28c33bc993760b35cc3479322af9d63d72ec191` ; ancien binaire préservé. Pas de replay de tests pour ce build. |

[Conception](design.md), [implémentation](implementation.md), [rapport des tests](tests.md). La baseline exacte [api.rs avant](before-api.rs.txt), SHA `823451d1a68c91e4985237d24934bd896c10a808b40df25844bff3b8a61c0b79`, vient de l'archive source interactive antérieure ; son hash correspond à celui vérifié avant RED. Les [sources après](after-api.rs.txt), [module d'authentification](after-auth.rs.txt) et [test final](after-tests.rs.txt) sont copiés pour relire le delta. Le test RED avant rustfmt et le test final ont leurs hashes distincts dans le rapport.

L'index SHA-256 vers tenant/rôle est construit au démarrage. Index, dictionnaire de masquage, politique et client Jev sont partagés par `Arc` pour éviter leur copie complète lors du clonage de l'état HTTP. Les tokens bruts ne sont plus conservés dans chaque `TenantState`. Une requête vérifie un seul champ Bearer, au plus 4096 octets sans espace blanc, puis un seul digest/lookup ; aucune promesse de temps constant, d'effacement mémoire ou de performance mesurée n'en découle. Configuration invalide : refus global503 (`new`) ou erreur de construction (`for_tenants`), jamais un index partiellement utilisable.

La sonde TCP vérifie la première ligne401 avec `Connection: close`. Elle ne prouve ni drainage, fermeture bornée, keep-alive, charge de connexions lentes, ni arrêt du processus avec SSE ouvert. Le transport peut déjà avoir reçu des octets ; la garantie corrigée concerne l'extraction applicative. Les refus Origin/URI et le routage précèdent toujours les guards. La suite complète, Docker/Podman, navigateur et appels modèles restent NOT_RUN pour ce lot ; leurs preuves antérieures ne sont pas transférées.
