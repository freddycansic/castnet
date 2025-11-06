<script lang="ts">
	import { onMount } from 'svelte';
	import cytoscape from 'cytoscape';
	import fcose from 'cytoscape-fcose';
	// import cise from 'cytoscape-cise';

	cytoscape.use(fcose);
	// cytoscape.use(cise);

	type Film = {
		id: number;
		title: string;
		year: number;
		popularity: number;
	};

	type Actor = {
		id: number;
		name: string;
		popularity: number;
		features: number;
	};

	type Role = {
		id: string;
		film_id: number;
		actor_id: number;
		character: string;
	};

	let cy: cytoscape.Core;
	let container: HTMLDivElement;

	onMount(async () => {
		instantiateGraph();
	});

	let title = '';
	let films: Film[] = [];
	let selectedFilmId: number | null = null;
	let selectedNode: number | null = null;

	async function searchFilms(title: string) {
		const res = await fetch(`http://localhost:3000/search/film?title=${encodeURIComponent(title)}`);
		films = await res.json();
	}

	async function addToGraph(film_id: number) {
		await fetch(`http://localhost:3000/graph/add/${encodeURIComponent(film_id)}`, {
			method: 'POST'
		});
		console.log('Adding to graph:', film_id);

		// TODO, return new objects from graph/add/ then only append those instead of rebuilding whole graph
		// Could use MERGE ON CREATE SET to determine which nodes are new
		refreshGraph();
	}

	async function instantiateGraph() {
		cy = cytoscape({
			container,
			selectionType: 'attitive',
			elements: fetchGraphElements(),
			style: [
				{
					selector: 'node[type="actor"]',
					style: {
						label: 'data(label)',
						'font-size': (actor) => {
							const features = actor.data('features') || 0;
							return 13 * Math.log(2 * features + 1);
						},
						'background-color': '#c4f0f1',
						'text-valign': 'center'
					}
				},
				{
					selector: 'node[type="film"]',
					style: {
						label: 'data(label)',
						'font-size': '16px',
						'background-color': '#dec4f1',
						color: '#000000',
						'text-valign': 'center'
					}
				},
				{
					selector: 'edge',
					style: {
						label: 'data(label)',
						'font-size': '8px',
						'curve-style': 'bezier',
						'target-arrow-shape': 'triangle',
						width: 1
					}
				}
			],
			layout: graphLayout()
		});

		cy.on('tap', 'node', (event: cytoscape.EventObject) => {
			const node = event.target;
			const connectedEdges = node.connectedEdges();
			const connectedNodes = connectedEdges.connectedNodes();

			const highlightNodes = connectedNodes.add(node);
			const highlightEdges = connectedEdges;

			const min_opacity = 0.2;

			cy.batch(() => {
				cy.nodes().style({
					'background-opacity': min_opacity,
					'text-opacity': min_opacity
				});
				cy.edges().style('opacity', min_opacity);

				highlightNodes.style({
					'background-opacity': 1.0,
					'text-opacity': 1.0
				});
				highlightEdges.style('opacity', 1.0);
			});
		});

		cy.on('tap', (event: cytoscape.EventObject) => {
			if (event.target === cy) {
				selectedNode = null;

				cy.batch(() => {
					cy.nodes().style({
						'background-opacity': 1.0,
						'text-opacity': 1.0
					});
					cy.edges().style('opacity', 1.0);
				});
			}
		});
	}

	type GraphResponse = {
		actors: Actor[];
		films: Film[];
		roles: Role[];
	};

	async function fetchGraphElements() {
		const res = await fetch('http://localhost:3000/graph');
		const { actors, films, roles }: GraphResponse = await res.json();

		const elements = [
			...actors.map((actor: Actor) => ({
				data: {
					id: actor.id,
					label: actor.name,
					popularity: actor.popularity,
					features: actor.features,
					type: 'actor'
				}
			})),
			...films.map((film: Film) => {
				const year = film.year ? ` (${film.year})` : '';

				return {
					data: { id: film.id, label: `${film.title}${year}`, type: 'film' }
				};
			}),
			...roles.map((role: Role) => ({
				data: {
					id: role.id,
					source: role.actor_id,
					target: role.film_id,
					label: role.character
				}
			}))
		];

		return elements;
	}

	async function refreshGraph() {
		cy.elements().remove();

		cy.add(await fetchGraphElements());

		cy.layout(graphLayout()).run();
	}

	function graphLayout() {
		return {
			name: 'fcose',
			quality: 'proof',
			randomize: false,
			animate: true,
			animationDuration: 1000,
			fit: true,
			padding: 50,
			nodeRepulsion: (node: cytoscape.NodeSingular) => {
				if (node.data('type') === 'actor') return 7000;
				if (node.data('type') === 'film') return 3000;
			},
			idealEdgeLength: 130,
			edgeElasticity: 0.45,
			gravity: 0.1,
			gravityRange: 3.0,
			gravityCompound: 1.0,
			gravityRangeCompound: 1.5,
			tile: true,
			tilingPaddingVertical: 15,
			tilingPaddingHorizontal: 15,
			nodeDimensionsIncludeLabels: true,
			uniformNodeDimensions: false
		};
	}
</script>

<input
	type="text"
	placeholder="Enter film title"
	bind:value={title}
	on:keyup={(e) => e.key === 'Enter' && searchFilms(title)}
/>
<button on:click={() => searchFilms(title)}> Search </button>

<div bind:this={container} style="width:100%; height:600px;"></div>

{#if films.length > 0}
	<select bind:value={selectedFilmId}>
		<option value={null} disabled selected>Select a film</option>
		{#each films as film (film.id)}
			<option value={film.id}>{film.title} ({film.year})</option>
		{/each}
	</select>
	<button on:click={() => selectedFilmId && addToGraph(selectedFilmId)}> Add to Graph </button>
{/if}
