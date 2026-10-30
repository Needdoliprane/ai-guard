# Revue indépendante — relais d'entrée HTTP Rust

29 octobre 2026. Revue en lecture seule des deltas `src/ingress.rs`, `src/lib.rs`, `tests/ingress.rs`, CLI ingress et arrêt dans `src/main.rs`, `tests/ingress_cli.rs` et les Compose Docker/Podman. L'option structurelle porte bien ACCEPTED dans `docs/engineering/DECISIONS.md`. Instructions AGENTS/RTK et contrats architecture/codage/gates/tests lus ; skills projet absents, aucun chargement prétendu. Aucun fichier source modifié, aucun moteur/conteneur/cloud, secret réel ou destruction utilisé pour cette revue.

## Verdict final du lot logiciel

**PASS après corrections et revalidation ciblée.** Deux P1 de disponibilité et deux P2 ont été identifiés puis traités. Aucun défaut bloquant restant ni contournement SSRF/authentification/rôle reproduit dans le lot relu. Les constats initiaux et leurs reproductions restent conservés ci-dessous ; les preuves de fermeture sont dans la section de revalidation.

Ce PASS porte sur le logiciel et les configurations relus. Il ne qualifie ni publication de ports, ni absence de route egress effective, ni Docker/Podman, ni le produit ou une release.

## Constats reproductibles

### P1 CLOSED — Un upload progressant lentement peut retenir toutes les places

Dans la version initialement relue, `src/ingress.rs:170-188` remet un timeout inter-chunks à chaque tour de lecture ; aucune durée maximale de réception de la requête n'existe. Le permit est déjà acquis à la ligne 238. Un client envoyant un octet avant chaque échéance conserve donc sa place très longtemps sans terminer son corps, même si le backend n'est jamais contacté. À 64 clients, les autres parcours, y compris `/healthz`, renvoient 429. La borne 256 KiB limite les octets, pas la durée de monopolisation.

Reproduction synthétique : `/tmp/ai-guard-review-ingress-probes.rs`, test `reviewer::reviewer_progressing_upload_retains_all_slots_without_absolute_deadline`. Copie exacte du module relu dans un scratch, même code HTTP, timeouts privés ramenés à 50 ms pour ne pas attendre les 30 s de production ; 64 sockets TCP envoient des corps chunkés inachevés, un octet toutes les 15 ms. Après plus de 300 ms, requête supplémentaire de santé → 429, backend_calls=0. Le test constate ensuite que la déconnexion libère bien les places. Exécution exit 0 : la reproduction est confirmée, ce n'est pas un PASS du critère de sûreté.

Correction nécessaire : borne totale de lecture de la requête indépendante du délai inter-chunks et du streaming SSE de réponse, avec régression de progression lente et libération des places.

### P1 CLOSED — La contre-pression aval suspend le timer du corps réponse

Dans la version initiale `src/ingress.rs:290-301`, le timer chunk est exécuté à l'intérieur du Stream retourné comme Body. Quand le client cesse de lire un export et que les buffers TCP sont pleins, Hyper cesse de poller ce Body. Le timer ne tourne alors plus ; le permit et le flux amont restent retenus sans borne de progression aval. Répéter ce comportement peut épuiser les 64 places. Un timeout total SSE ne constitue pas une correction acceptable.

Reproduction : même scratch, `reviewer_response::reviewer_nonreading_export_client_holds_slot_past_chunk_deadline`. Le test utilise une place au lieu de 64 pour isoler le mécanisme ; backend exportant 1024 chunks de 64 KiB, client TCP ne lisant aucun corps, délai chunk de 50 ms. Après 500 ms, place encore occupée et production amont immobilisée à 28/1024 chunks. La fermeture client libère la place. Exécution exit 0, reproduction confirmée. Aucun gigaoctet n'est alloué : la production est bornée par la contre-pression observée.

Correction nécessaire : surveiller la progression aval indépendamment du polling du Body, avec tampon borné, arrêt de l'amont et libération sur délai/cancellation, tout en conservant des SSE actifs sans limite totale arbitraire.

### P2 CLOSED — Le healthcheck Compose ignore le trajet dashboard

Les Compose initialement relus sondent seulement `:8787/healthz` pour ingress, malgré les deux listeners publiés. Une panne dashboard après `depends_on` initial ne change pas la réussite de cette sonde. La disponibilité globale du relais pourrait donc rester verte alors que 4000 échoue. Constat établi par inspection, pas par lancement de conteneurs. Correction nécessaire : sonde Rust bornée couvrant les deux trajets, avec test où un seul backend est indisponible.

Revue séparée CLI/Compose déléguée puis confrontée au code : `/tmp/ai-guard-review-ingress-deploy.md`.

### P2 CLOSED — Premier correctif : la fin amont libère trop tôt la garde normale

La première pompe de correction possédait seule le permit et le laissait tomber dès l'EOF amont, alors que le Body aval pouvait encore contenir un chunk et n'avoir jamais été consommé. Cela ne respectait pas le contrat de place détenue durant le Body. La voie d'erreur doit libérer en cas de blocage ; la voie normale doit conserver sa garde jusqu'à EOF ou drop aval.

Reproduction : `/tmp/ai-guard-review-ingress-recheck.rs`, `reviewer_lifetime::reviewer_upstream_eof_releases_slot_before_downstream_body_is_polled` → exit 0, reproduction confirmée. Avec une place, un backend de petit corps complet et un appel direct du même handler HTTP, une réponse 200 est tenue 30 ms sans aucun poll de son Body ; la place est déjà disponible. Ce cas n'exige ni gros buffers ni charge. Le défaut est transmis avant clôture de la correction.

## Critères vérifiés positivement

- Destination fixée au démarrage ; validation origin HTTP(S), refus userinfo/path/query/fragment, vérification de la forme brute avant normalisation. Les URI absolues, CONNECT, TRACE et upgrades sont refusés. `set_path`/`set_query` n'emploient pas une autorité fournie par HTTP.
- `Host`, Forwarded/X-Forwarded-*/X-Real-IP entrants retirés ; filtres hop-by-hop fixes et champs nommés par toutes les valeurs Connection dans les deux directions. Bearer, Origin, Sec-Fetch, Last-Event-ID, statuts, Content-Type et corps sont relayés selon le contrat.
- Client upstream sans proxy d'environnement, redirect ni retry automatique ; aucune option désactivant TLS n'est ajoutée. Pas de corps ni credential journalisé dans le module. Les erreurs réseau sont génériques.
- Limites URI de 4096 octets et corps de 256 KiB appliquées avant contact backend, y compris corps chunkés ; lecture incomplète refusée. Les places sont détenues durant les corps et se libèrent à la déconnexion dans les probes. Les deux P1 initiaux ont conduit à compléter les délais inter-chunks par les contrôles indépendants décrits ci-dessous.
- `/healthz` traverse réellement sa destination, ne fabrique pas une santé locale. La sonde globale Compose constitue le P2 distinct.
- Probe indépendant avec le vrai gateway Axum et SQLite, à travers ingress : sans Bearer 401 ; viewer lecture 200 ; viewer écriture 403 ; runner avec Origin externe 403 sans événement ; runner autorisé 200 avec audit_seq 1 durable. Test `reviewer_auth::reviewer_gateway_rejects_unauthenticated_viewer_write_and_cross_site_through_ingress` → 1 PASS, 0 FAIL, 0 ignoré.
- CLI par lecture : deux routeurs distincts, deux binds avant service, erreurs propagées, signal partagé et drainage de 5 s. La revue déléguée a relancé `cargo test --locked --test ingress_cli` : 1 PASS, 0 ignoré, 5,73 s avec SSE actif.
- Compose par lecture : seul ingress rejoint control+publication et porte des ports127.0.0.1 ; gateway/dashboard restent sur control interne. Ingress ne monte ni secret, credential, volume persistant, projet ni socket ; profil non root/read_only/cap_drop/no-new-privileges conservé. Ce constat YAML ne prouve pas l'absence de route réelle en conteneur.

## Preuves et limites

Les scratches ont été compilés avec rustc édition 2024 `--test`, dépendances déjà construites dans `target/debug/deps`, sans mutation du dépôt. Les trois probes initiaux reviewer sont lancés individuellement avec `--exact ... --nocapture`. Les timeouts rapides sont privés aux scratches ; aucune configuration publique de test n'a été ajoutée.

Les neuf tests HTTP du module et les résultats root `/tmp/ai-guard-ingress-cli-green.log` ont été lus ; ils ne couvraient pas les deux cas de progression lente reproduits ici. Le test CLI actuel emploie un seul backend pour les deux listeners : la source est correctement câblée, mais cette régression ne détecte pas une inversion future ; deux backends identifiables amélioreraient directement cette preuve.

## Revalidation des correctifs

Le code final a été recopié dans `/tmp/ai-guard-review-ingress-final.rs`, puis les probes reviewer y ont été ajoutés. Son préfixe a été extrait dans `/tmp/ai-guard-review-ingress-final-core.rs` ; `cmp src/ingress.rs /tmp/ai-guard-review-ingress-final-core.rs` retourne exit 0. Les tests portent donc sur une copie identique du module final, avec ses fonctions privées et ses délais courts, sans modification des sources.

- **Upload : CLOSED.** `Timeouts.request_body=30s` borne la réception complète indépendamment du délai inter-chunks. Timeout →408 avant backend et le permit est libéré par sortie de handler. La régression de progression lente conserve 63 autres places et prouve la restitution de la dernière.
- **Aval : CLOSED.** Pompe tokio indépendante, canal de capacité 1, délais sur lecture amont et envoi au canal ; fermeture receiver surveillée. Une lease partagée avec le Body libère idempotemment sur erreur, timeout, annulation ou abandon de tâche ; l'erreur terminale persiste hors du canal, même plein. Aucun timeout total n'est imposé aux SSE actifs.
- **Fin normale : CLOSED.** `BodyLease` possède la garde dès création, y compris avant le premier poll. EOF amont n'entraîne pas de libération normale anticipée. L'attente finale de consommation est elle-même bornée : une dernière livraison bloquée peut expirer et rendre un corps tardif en erreur, sans être présentée comme un export complet. Mutex court sans await ; Notify conserve une notification de fin précoce.
- **Santé : CLOSED.** `healthcheck --url` répétable requis, au plus 8 destinations ; validation URL sans userinfo/query/fragment, appels concurrents avec délai total 3s et corps maximal 4KiB, sans proxy ni redirection. Succès seulement si toutes les destinations répondent avec status ok. Les deux Compose sondent maintenant 8787 et 4000 ; leurs frontières réseau et montages ne changent pas.

Exécutions indépendantes finales :

```text
/tmp/ai-guard-review-ingress-final reviewer_ --nocapture --test-threads=1
2 passed; 0 failed; 0 ignored; 6 filtered out

/tmp/ai-guard-review-ingress-final tests:: --nocapture --test-threads=1
6 passed; 0 failed; 0 ignored; 2 filtered out

target/debug/deps/ingress_cli-316e8f4f5050a5ae --exact healthcheck_fails_if_either_destination_is_down_or_oversized --nocapture
1 passed; 0 failed; 0 ignored; 1 filtered out
```

Les deux probes reviewer vérifient à nouveau le gateway réel à travers le module corrigé, puis un petit corps déjà terminé côté amont : place encore tenue avant consommation normale ; libération après consommation ; seconde réponse jamais pollée expirant et échouant en lecture tardive. Les six unités couvrent les trois deadlines de base et les régressions upload, aval non pollé et garde EOF/drop. La sonde multi-URL est rejouée sans le test SSE déjà constaté, ni nouvelle suite globale inutile.

Le rapport auteur `/tmp/ai-guard-ingress-report.md` conserve les RED puis GREEN des trois défauts core, ses 6 unités/6 intégrations et Clippy ciblé. Les journaux globaux root `...-final-clippy.log`, `...-final-release.log`, `...-final-test.log` sont attribués à root ; ils ne sont pas comptés comme des exécutions du reviewer. L'essai CLI indépendant délégué et les journaux root constatent l'arrêt SIGTERM avec SSE et le drainage de 5s.

SHA256 du périmètre final relu :

```text
702329d51cf52e36d2dc0b44b1d044ca6657bfd39222a3cdaba5c0e0b3b63559  src/ingress.rs
c8807a5745f2a04f013703d5501ef8436eb6fbd7f2cdb6c113089a814fe52d88  src/lib.rs
3076d326ddd7aa1f9e87ac0b2f66987a2fd3677cd3b7d553dc8c7ffa748061dc  tests/ingress.rs
50b3bf7f9f9fd436dffc1d3a78dfa6db3890dab81e1aa0fd6bc6b34198ec9b7e  src/main.rs
e8638365b83c3a00feb39ec3c3a1d62c4dc1c47f9dbe93f57fdca3884c88378c  tests/ingress_cli.rs
a02aedebd66482748336559001f441ddb58aecd6b64d3c1db5279afdc10673b5  deploy/docker/compose.yaml
b2dd6ad53fa32c5bd572df51259b8bac6a0301b2c05f1bfc5ee04af5f4fc6f07  deploy/podman/compose.yaml
```

- PASS : revue logicielle finale, bornes et annulation corrigées, transport/auth et sonde multi-trajet ciblés décrits ci-dessus.
- FAIL : défauts initiaux conservés comme reproductions historiques, tous fermés dans ce lot ; aucun échec restant observé sur les vérifications finales.
- NOT_RUN : suites globales supplémentaires par le reviewer, TLS réel/PKI, effets Docker/Podman, cloud, worker, charge production.
- BLOCKED : qualification production globale selon les gates ouverts ; aucune clôture de ces gates n'est déduite de ce lot.
- NOT_APPLICABLE : publication, facturation et intervention sur une infrastructure externe.

Ce rapport ne qualifie pas la release ou le produit. La sortie réseau appartient seulement au composant ingress accepté ; elle ne réduit aucune obligation d'isolation des services internes, d'identité ou d'audit.
