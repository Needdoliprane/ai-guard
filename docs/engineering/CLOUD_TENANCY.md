# Identité et stockage par client

Le [contrat d'architecture](ARCHITECTURE_CONTRACT.md) fixe la cible complète. L'identité runner/viewer et l'isolement des API d'audit par base SQLite ont été vérifiés sur le précédent candidat. Le lot courant ajoute le registre, les quotas, l'allocation et le dashboard partagé ; ses tests logiciels ne remplacent pas la qualification intégrée du nouveau broker. Le rôle worker, le relais métier et l'adaptateur Kubernetes restent absents ; les preuves datées sont dans [CURRENT_STATE.md](CURRENT_STATE.md).

Le mainteneur demande également un [SSO OIDC compatible Keycloak et PKCE](SSO_OIDC.md),31octobre2026. Le code actuel utilise les clés runner/viewer, pas un validateur JWT/OIDC. La future identité humaine doit être vérifiée côté Rust puis liée à des memberships/permissions administrés côté serveur. Un realm, un client public, un email ou un `tenant_id` de requête n'est pas à lui seul une autorisation ; une sélection de contexte ne peut choisir que parmi les associations déjà autorisées. Les identités des brokers/workers restent distinctes des sessions humaines. La configuration SSO et sa matrice A/B sont obligatoires à implémenter et qualifier avant de déclarer ce parcours cloud livré.

## Configuration du contrôle de confiance

Le gateway accepte `--tenants-file /chemin/tenants.json`. Cette option est incompatible avec `--data`, `--runner-token-file`, `--viewer-token-file` et les quatre options de quota locales. Sans registre, la configuration locale historique est conservée avec identité `local`.

```json
{
  "version": 1,
  "tenants": [
    {
      "tenant_id": "tenant-a",
      "data": "data/a.sqlite",
      "runner_token_file": "secrets/a-runner",
      "viewer_token_file": "secrets/a-viewer",
      "worker_quota": {
        "version": 1,
        "max_active_workers": 3,
        "max_cpu_millis": 6000,
        "max_memory_bytes": 3221225472,
        "max_timeout_seconds": 3600
      }
    },
    {
      "tenant_id": "tenant-b",
      "data": "data/b.sqlite",
      "runner_token_file": "secrets/b-runner",
      "viewer_token_file": "secrets/b-viewer"
    }
  ]
}
```

Les chemins sont absolus ou relatifs au répertoire du registre. Le JSON ne contient aucune valeur de secret ; les champs inconnus et les versions inconnues sont refusés. Les secrets ont 32 à 512 caractères sans espace ; chaque rôle et chaque tenant possède un secret distinct. Le registre est borné à 1 Mio et les fichiers de credentials sont lus avec une limite, en refusant les liens finaux et fichiers spéciaux sur Unix. L'opérateur protège le registre et ses répertoires ; ces fichiers appartiennent au contrôle de confiance.

`worker_quota` est facultatif et prend les valeurs de l'exemple par défaut. Les plafonds de configuration acceptés sont 100 workers, 200 000 milliCPU, 100 Gio et 86 400 secondes ; la durée doit être positive, les trois capacités peuvent être nulles pour refuser toute nouvelle admission. Ces bornes ne sont pas une qualification de capacité. Une nouvelle admission réserve le nombre, le CPU et la mémoire dans la même transaction que l'audit. La durée limite l'admission, mais le contrôle de durée après crash du broker reste un écart ouvert. Changer les quotas n'arrête pas les workers déjà admis. En mode local, les options correspondantes sont `--worker-limit`, `--worker-cpu-millis`, `--worker-memory-bytes`, `--worker-max-seconds`.

Une identité authentifiée sélectionne exactement une base. Aucun `tenant_id` de requête ne remplace cette sélection. Les IDs projet/session/agent ne sont que des filtres dans cette base. Le runner peut écrire et lire ; le viewer peut lire. Aucun credential worker ne doit réutiliser l'un de ces secrets. Le rôle worker reste absent et un jeton inconnu est refusé.

`GET /api/v1/identity` renvoie uniquement `{tenant_id,role}` correspondant au jeton. Toutes les routes d'audit disposent du préfixe `/api/v1` ; les routes `/api` restent des alias compatibles. Les flux, exports, intégrité, décisions et compteurs utilisent la base authentifiée ; les limites de débit et SSE sont distinctes par tenant.

## Liaison durable et compatibilité

La première ouverture lie transactionnellement la base à son tenant dans `tenant_binding`, version 1. Une réouverture sous une autre identité ou une version inconnue est refusée. Une base historique non vide sans liaison n'est adoptée que par `local`, jamais automatiquement par un nouveau client. Les événements, hashes et séquences restent conservés. Le déplacement des données d'un ancien client vers un autre exige une migration explicite, non implémentée par ce lot.

Les fichiers SQLite/WAL doivent rester sur un stockage local fiable au contrôle. Cette évolution n'introduit ni partage concurrent d'une base entre clients, ni haute disponibilité. Une base neuve utilise le schéma de contrôle v2 et commence par un événement de genesis audité ; une base v1 existante exige la [migration explicite sauvegardée](../operations.md) avant les API workers. Cette migration conserve les anciens événements ; elle ne restaure pas un inventaire de workers exécutés par l'ancien broker. La restauration d'un état v2 contenant des claims de start exige encore une procédure de fencing/réconciliation qualifiée ; restaurer une sauvegarde ne donne aucun droit de rejouer un start.

## Déploiement et limites

### Dashboard unifié obligatoire

Choix du mainteneur,30octobre2026 : un seul accès Web par client regroupe tous ses agents, sous-agents et workers sur ses brokers/hôtes/runtimes. A voit tout A selon ses permissions, B tout B ; aucun dashboard par worker à ouvrir. Le control plane centralise registre, quotas, événements et usage, sans multiplier les plafonds par broker. L'UI n'accède ni aux moteurs ni aux credentials runner. Provenance, autorité et fraîcheur restent visibles ; une collecte partielle ou un broker déconnecté n'est pas une vue complète. Authentification des brokers distants, transports, reprise et charge100 restent à livrer/qualifier ; le mode partagé actuel ne les prouve pas.

Le broker reste Rust dans le périmètre de confiance ; aucun moteur n'est accessible depuis les workers. Pour un gateway conteneurisé, monter uniquement les bases et credentials nécessaires dans le contrôle, sans socket moteur ni projet original. Les Compose historiques restent mono-client ; ils ne déploient pas la recette A1/A2/B1. Le dashboard conserve son mode viewer fixe ; `--shared-tenants` permet les viewers du registre authentifiés par le gateway, sans fichier de credentials monté dans le dashboard. Chaque utilisateur consulte son tenant ; aucune vue administrateur inter-clients n'est livrée. Les clés runner restent refusées côté dashboard.

Les tests logiciels A/B et le redémarrage d'un vrai processus HTTP apportent une preuve de routage/persistance. Ils ne prouvent ni l'isolation de volumes/réseau de workers réels, ni le cloud, ni cent workers. Les limites actuelles et commandes exécutées sont dans [VALIDATION.md](VALIDATION.md).
