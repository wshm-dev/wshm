import type { Role } from './api';

/**
 * RBAC matrix.
 *
 * Each capability is mapped to the minimum role required. The sidebar
 * uses this for route hiding; pages use it to disable buttons. Backend
 * enforcement is the source of truth — this is a UX layer.
 */
const RANK: Record<Role, number> = {
	viewer: 0,
	member: 1,
	operator: 2,
	admin: 3
};

const PAGE_REQUIREMENTS: Partial<Record<string, Role>> = {
	'/actions': 'member',
	'/revert': 'operator',
	'/backups': 'operator',
	'/settings': 'admin',
	'/users': 'admin'
};

/** Capability → minimum role required. */
export const CAN: Record<string, Role> = {
	syncIncremental: 'member',
	syncFull: 'operator',
	triageManual: 'member',
	analyzeManual: 'member',
	mergeManual: 'operator',
	closeManual: 'operator',
	revertPreview: 'member',
	revertApply: 'operator',
	createBackup: 'operator',
	restoreBackup: 'operator',
	deleteBackup: 'admin',
	addRepo: 'admin',
	deleteRepo: 'admin',
	manageSecrets: 'admin',
	revealSecret: 'admin',
	manageUsers: 'admin',
	activateLicense: 'admin',
	editAiProvider: 'admin'
};

export function hasRole(actual: Role | undefined, required: Role): boolean {
	if (!actual) return false;
	return RANK[actual] >= RANK[required];
}

export function canAccessRoute(role: Role | undefined, href: string): boolean {
	const required = PAGE_REQUIREMENTS[href];
	if (!required) return true;
	return hasRole(role, required);
}

export function can(role: Role | undefined, capability: keyof typeof CAN): boolean {
	return hasRole(role, CAN[capability]);
}

export function isAdmin(role: Role | undefined): boolean {
	return hasRole(role, 'admin');
}

export function isOperatorOrAbove(role: Role | undefined): boolean {
	return hasRole(role, 'operator');
}

export function isMemberOrAbove(role: Role | undefined): boolean {
	return hasRole(role, 'member');
}
