import type { Agent } from '$lib/api';

class AgentsState {
	list = $state<Agent[]>([]);

	setList(agents: Agent[]) {
		this.list = agents;
	}

	updateAgent(updated: Partial<Agent> & { id: string }) {
		this.list = this.list.map((a) => (a.id === updated.id ? { ...a, ...updated } : a));
	}

	addAgent(created: Agent) {
		if (!this.list.some((a) => a.id === created.id)) {
			this.list = [...this.list, created];
		}
	}

	removeAgent(id: string) {
		this.list = this.list.filter((a) => a.id !== id);
	}
}

export const agentsState = new AgentsState();
