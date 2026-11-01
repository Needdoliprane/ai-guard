# Inférence locale — bilan daté du 1er novembre 2026

Le mainteneur demande que les essais Qwen auxiliaires restent locaux et ignorés. Instruments, captures et archives sont conservés sur le poste ; leur retrait du suivi n'efface pas les fichiers ou l'historique. Le [guide du contexte IA locale](../../../../setup/local-ai/README.fr.md) porte le contrat utilisateur, distinct de cette preuve datée.

Quatre cas de transport ont reproduit le défaut de lecture fragmentée du probe, puis confirmé sa correction et les refus d'un corps tronqué et d'un serveur silencieux. Un premier choix d'image incorrect a échoué127 avant exécution ; ce n'était pas le RED. Une seule inférence synthétique Chat Completions a ensuite réussi sur Qwen2.5-0.5B-Instruct Q4_K_M. Réponse639octets SHA256 `b4c87043f33912772fb5708f98b58b066b0ea5c192db38b37cc9f62c14b5da23`, JSON validé indépendamment ; tokens40/3/43 déclarés par le serveur, sans facturation.

Pic cgroup788877312 octets sous plafond999997440, sans OOM/swap. Arrêt du même serveur confirmé `exited 0`, objets conservés. Le validateur privé avait des tolérances JSON ; la revue stricte du même échantillon, sans nouvelle inférence, borne le PASS. Le premier incident126 de permission est seulement rapporté, sans artefact durable distinct retrouvé.

Les détails précédemment publiés restent dans le commit `71324a6f5fb2361a9c08b214ede7a601aca95d97`. Le FAIL historique du modèle comme juge de sécurité est conservé dans le [bilan du30octobre](../../2026-10-30/local-judge/README.md). Aucun raccord Codex/Claude, adaptateur produit, TLS, entreprise, Podman, performance générale ou gate production n'est qualifié par cette salutation.
