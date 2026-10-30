# Revue ciblée — préparation et reconstruction broker

31 octobre 2026, worktree broker-service, base `7098e204a91182229c1a67bc8017271cb3d20918`. Contrat actif IMPLEMENTATION_PLAN, contrat privé de soumission et instructions applicables lus. Lecture seule ; aucun Cargo, modèle, moteur ou fichier source modifié par le reviewer. Note privée 0600.

Gel1 vérifié : runner.rs `ba72d730ee8d611196bf1130ab8bfc9bd56ad1d3b7f9ea5996b1221daa5efac2`, runner/broker.rs `83bc1e6ab623bc6cc2b6314f0e0b32c4007b0c4c1bda02a0e693f297d1a8930b`.

## Verdict source

**Aucun constat bloquant confirmé au gel lu.** La préparation neuve est distincte de la reconstruction et ne crée ni droit ni identité authentifiée. Clôture des tests/documents encore attendue.

- `prepare_broker_submission` refuse UUID nil, approve_hash et interactive avant toute IO (`broker.rs:66-93`). Les littéraux sont bornés :64,4096octets chacun,256KiB cumulés. Les erreurs publiques sont fixes et n'exposent pas chemins ou valeurs.
- Le wrapper legacy choose_plan conserve ses branches et ses identités. Le cœur choisit une fois les mêmes options/projection/résolution ; seul l'opération UUID broker est imposé avant création du RunPlan. L'objet final ne conserve qu'une intention construite une fois, un binding retenu et config ; pas de deuxième plan mutable.
- Manuel : credential_source est déplacé depuis le vrai RunPlan, pas rouvert. La canonicalisation commune vérifie les variables et les littéraux contrôle sur le handoff. Fournisseur : freeze réutilise le fichier déjà lu après write/sync et retourne son vrai PrivateFile ; ID/nom/digest et provider_input/résolution/argv restent ensemble. Le nouveau chemin passe Some(control_literals), donc n'appelle pas control_secrets/config ou read_token. Le wrapper legacy conserve sa lecture historique.
- Le Directory config retenu traverse la préparation ; une référence Path ne remplace pas ses FD. Les sources et snapshots sont privés et les types PreparedSubmission/BrokerRunState/BrokerCredentials sont non Serde avec Debug opaque.
- `BrokerRunState::from_job` (`157-184`) est pure : intention déjà validée, opération/session/options/version et référence provider reprises exactement ; current_request et managed clonés depuis le checkpoint courant ; credential_source=None. validate_provider_plan est pure (données/résolution/argv), sans Auto, FS, token ou reconstruction d'intention. Approbation et hash ne sont pas remis à zéro. Accepter des données V1 lisibles ne vaut ni migration d'ancien managed ni permission d'exécuter.
- `credentials_from_job` (`253-334`) vérifie config contre l'intention puis n'utilise que binding_source retenu. None retourne sans credential ; manuel reparse les bytes durables ; fournisseur recoupe ID/nom/source/stored digest avec l'intention. Noms et hash du handoff sont recoupés avant création du Snapshot minimal. Aucun appel au chemin original tourné, providers.json, frozen_provider_credentials ou secret contrôle.
- BrokerCredentials conserve un clone Arc du PrivateFile durable, le Snapshot et config. validate contrôle les ancêtres, l'identité du nom et les bytes réels par persistent_revalidate/read_at sur le FD retenu ; aucun second FD vers le fichier source. La durée de vie du handoff ne dépend donc pas du maintien de job/prepared chez le caller. La validation doit précéder l'usage du chemin ; elle ne prétend pas fence un acteur même UID/root.

## Oracles à compléter par Luna

La rotation du fichier fournisseur original ne distingue pas, seule, le bon replay Journal d'un fallback au record legacy encore intact. Demande transmise : retirer aussi providers.json et credential-bindings/<id>.json après accept, puis retrouver les mêmes bytes durables. Pour manuel/fournisseur, vérifier le fichier credentials.path/hash contre handoff_sha, altérer ce snapshot puis attendre validate Err ; les seuls secrets() cachés ne prouvent pas le handoff fichier. Drop(job/prepared) avant validate permet de vérifier la propriété FD. Aucun test exécuté ici et aucun PASS anticipé.

Le futur service doit authentifier avant profil/FS, préparer uniquement après ReserveResult::New et refuser le batch interactif avant toute reconstruction de handoff. L'enforcer/IPC/CLI, les archives de workspace et les effets runtime restent hors de cette couture ; pas de source ou droit rechargé implicitement au replay.
