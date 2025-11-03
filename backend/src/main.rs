use std::{
    collections::HashMap,
    sync::{Arc, LazyLock},
};

use axum::{
    Router,
    extract::{Path, Query, State},
    http::StatusCode,
    response::Json,
    routing::{get, post},
};
use chrono::Datelike;
use futures::TryStreamExt;
use neo4rs::{
    BoltNull, Config, ConfigBuilder, EndNodeId, Graph, Node, Relation, StartNodeId, query,
};
use reqwest::{Client, Method, RequestBuilder, Response};
use serde::{Deserialize, Serialize};
use serde_json::{Number, Value};
use tokio::sync::Semaphore;
use tower_http::cors;

#[derive(Clone)]
struct Tokens {
    tmdb_read_access_token: String,
    neo4j_username: String,
    neo4j_password: String,
}

#[derive(Clone)]
struct AppState {
    graph: Graph,
    max_connections: usize,
    tokens: Tokens,
    api_client: Client,
}

impl AppState {
    async fn new() -> Self {
        let tokens = Tokens {
            tmdb_read_access_token: dotenv::var("TMDB_READ_ACCESS_TOKEN").unwrap(),
            neo4j_username: dotenv::var("NEO4J_USERNAME").unwrap(),
            neo4j_password: dotenv::var("NEO4J_PASSWORD").unwrap(),
        };

        let api_client = Client::new();

        let graph = create_graph(&tokens).await;

        Self {
            graph,
            max_connections: 16, // Same as neo4rs, but this value is inaccessible
            tokens,
            api_client,
        }
    }

    fn api_get_request(&self, url: &str) -> RequestBuilder {
        self.api_client
            .get(url)
            .header(
                "Authorization",
                format!("Bearer {}", self.tokens.tmdb_read_access_token.clone()),
            )
            .header("accept", "application/json")
    }
}

#[derive(Serialize)]
struct Actor {
    id: u64,
    name: String,
    popularity: f32,
    features: u32,
}

#[derive(Serialize)]
struct Role {
    id: String,
    actor_id: u64,
    film_id: u64,
    character: String,
}

#[derive(Serialize)]
struct Film {
    id: u64,
    title: String,
    year: Option<i32>,
    popularity: f64,
}

async fn create_graph(tokens: &Tokens) -> Graph {
    let uri = "neo4j://127.0.0.1:7687";

    let graph_config = ConfigBuilder::default()
        .uri(uri)
        .user(&tokens.neo4j_username)
        .password(&tokens.neo4j_password)
        .build()
        .unwrap();

    let graph = Graph::connect(graph_config.clone()).await.unwrap();

    graph
        .run(query(
            "CREATE CONSTRAINT film_id_unique IF NOT EXISTS
            FOR (f:Film)
            REQUIRE f.id IS UNIQUE;",
        ))
        .await
        .unwrap();

    graph
        .run(query(
            "CREATE CONSTRAINT actor_id_unique IF NOT EXISTS
            FOR (a:Actor)
            REQUIRE a.id IS UNIQUE;",
        ))
        .await
        .unwrap();

    graph
}

#[tokio::main]
async fn main() {
    let state = AppState::new().await;

    let cors = cors::CorsLayer::new()
        .allow_origin(cors::Any)
        .allow_methods([Method::GET])
        .allow_headers(cors::Any);

    let app = Router::new()
        .route("/search/film", get(search_film))
        .route("/graph", get(get_graph))
        .route("/graph/add/{film_id}", post(add_film))
        .layer(cors)
        .with_state(state);

    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000").await.unwrap();
    axum::serve(listener, app).await.unwrap();
}

#[axum::debug_handler]
async fn search_film(
    Query(params): Query<HashMap<String, String>>,
    State(state): State<AppState>,
) -> Result<Json<Vec<Film>>, StatusCode> {
    let title_query = params.get("title").ok_or(StatusCode::BAD_REQUEST)?;

    let film_list_response = state
        .api_get_request("https://api.themoviedb.org/3/search/movie")
        .query(&[("query", &title_query)])
        .send()
        .await
        .map_err(|err| err.status().unwrap())?;

    let mut results = film_list_response.json::<Value>().await.unwrap()["results"]
        .as_array()
        .unwrap_or(&vec![])
        .iter()
        .filter_map(|film| {
            let year = film
                .get("release_date")?
                .as_str()
                .and_then(|date| chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d").ok())
                .map(|parsed_date| parsed_date.year());

            Some(Film {
                id: film.get("id")?.as_u64()?,
                title: film.get("title")?.as_str()?.to_string(),
                year,
                popularity: film.get("popularity")?.as_f64()?,
            })
        })
        .collect::<Vec<_>>();

    // Sort by descending popularity
    results.sort_by(|film_a, film_b| film_b.popularity.total_cmp(&film_a.popularity));

    println!("Searched for \"{title_query}\".");

    Ok(Json(results))
}

async fn add_film(Path(film_id): Path<u64>, State(state): State<AppState>) {
    let film_response = state
        .api_get_request(format!("https://api.themoviedb.org/3/movie/{film_id}").as_str())
        .send()
        .await
        .unwrap();

    let film_json = film_response.json::<Value>().await.unwrap();

    let title = film_json["title"].as_str().unwrap().to_string();
    let film_popularity = film_json["popularity"].as_f64().unwrap();
    let year = film_json["release_date"]
        .as_str()
        .and_then(|date| chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d").ok())
        .map(|parsed_date| parsed_date.year());

    let cast_response = state
        .api_get_request(format!("https://api.themoviedb.org/3/movie/{film_id}/credits").as_str())
        .send()
        .await
        .unwrap();

    let cast_json = cast_response.json::<Value>().await.unwrap();

    let cast_list = cast_json["cast"].as_array().unwrap();

    let mut create_actor_handles = Vec::with_capacity(cast_list.len());
    let semaphore = Arc::new(Semaphore::new(state.max_connections));

    const MIN_POPULARITY: f64 = 0.8;
    let actors = cast_list
        .into_iter()
        .filter(|actor| actor.get("known_for_department").unwrap().as_str() == Some("Acting"))
        .filter(|actor| actor.get("popularity").unwrap().as_f64().unwrap() > MIN_POPULARITY)
        .filter(|actor| actor.get("adult").unwrap().as_bool().unwrap_or(false) == false);

    for actor in actors {
        let graph = state.graph.clone();
        let actor_id = actor.get("id").unwrap().as_i64().unwrap();
        let actor_name = actor.get("name").unwrap().as_str().unwrap().to_string();
        let actor_popularity = actor.get("popularity").unwrap().as_f64().unwrap();
        let character = actor
            .get("character")
            .unwrap()
            .as_str()
            .unwrap()
            .to_string();
        let role_id = actor
            .get("credit_id")
            .unwrap()
            .as_str()
            .unwrap()
            .to_string();

        let title = title.clone();
        let year = year.clone();

        let semaphore = semaphore.clone();

        let handle = tokio::spawn(async move {
            let _ = semaphore.acquire_owned().await.unwrap();

            let create_actor_query = query(
                "
                MERGE (a:Actor {id: $actor_id})
                ON CREATE
                    SET a.name = $name,
                        a.popularity = $actor_popularity,
                        a.features = 1
                ON MATCH
                    SET a.features = coalesce(a.features, 0) + 1

                MERGE (f:Film {id: $film_id})
                ON CREATE
                    SET f.title = $title,
                        f.popularity = $film_popularity,
                        f.year = $year

                MERGE (a)-[r:ROLE {id: $role_id}]->(f)
                ON CREATE
                    SET r.character = $character

                RETURN a, f, r",
            )
            .param("actor_id", actor_id)
            .param("name", actor_name)
            .param("actor_popularity", actor_popularity)
            .param("film_id", film_id as i64)
            .param("title", title)
            .param("film_popularity", film_popularity)
            .param("character", character)
            .param("role_id", role_id)
            .param("year", year);

            graph.run(create_actor_query).await.unwrap();
        });
        create_actor_handles.push(handle);
    }

    futures::future::join_all(create_actor_handles).await;

    println!("Added film \"{title}\" to graph.");
}

#[derive(Serialize)]
struct GraphResponse {
    actors: Vec<Actor>,
    films: Vec<Film>,
    roles: Vec<Role>,
}

#[axum::debug_handler]
async fn get_graph(State(state): State<AppState>) -> Json<GraphResponse> {
    let actors = state
        .graph
        .execute(query("MATCH (a:Actor) RETURN a;"))
        .await
        .unwrap()
        .into_stream()
        .map_ok(|row| {
            let actor: Node = row.get("a").unwrap();
            Actor {
                id: actor.get("id").unwrap(),
                name: actor.get("name").unwrap(),
                popularity: actor.get("popularity").unwrap(),
                features: actor.get("features").unwrap(),
            }
        })
        .try_collect::<Vec<Actor>>()
        .await
        .unwrap();

    let films = state
        .graph
        .execute(query("MATCH (f:Film) RETURN f;"))
        .await
        .unwrap()
        .into_stream()
        .map_ok(|row| {
            let film: Node = row.get("f").unwrap();
            Film {
                id: film.get("id").unwrap(),
                title: film.get("title").unwrap(),
                year: film.get("year").unwrap(),
                popularity: film.get("popularity").unwrap(),
            }
        })
        .try_collect::<Vec<Film>>()
        .await
        .unwrap();

    let roles = state
        .graph
        .execute(query("MATCH (a:Actor)-[r:ROLE]->(f:Film) RETURN a, r, f;"))
        .await
        .unwrap()
        .into_stream()
        .map_ok(|row| {
            let actor: Node = row.get("a").unwrap();
            let role: Relation = row.get("r").unwrap();
            let film: Node = row.get("f").unwrap();

            Role {
                id: role.get("id").unwrap(),
                actor_id: actor.get("id").unwrap(),
                film_id: film.get("id").unwrap(),
                character: role.get("character").unwrap(),
            }
        })
        .try_collect::<Vec<Role>>()
        .await
        .unwrap();

    println!(
        "Got graph, actors: {}, films: {}, roles: {}",
        actors.len(),
        films.len(),
        roles.len()
    );

    Json(GraphResponse {
        actors,
        films,
        roles,
    })
}
