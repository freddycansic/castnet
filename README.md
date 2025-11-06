# Castnet

A net of casts. Build a graph of actors and films, and visualise common actors.

## Running

### Neo4j

This is the graph database used to back the back of the backend.

Follow these steps to install local instance of neo4j.

https://neo4j.com/docs/operations-manual/current/installation/linux/tarball/#unix-console

Start neo4j with

```
sudo -u neo4j /opt/neo4j/bin/neo4j console
```

Test you can log in at http://localhost:7474/browser/

### Backend

```
cd backend
```

```
cargo run
```

### Frontend

```
npm install // (I think?)
```

```
npm run dev
```
