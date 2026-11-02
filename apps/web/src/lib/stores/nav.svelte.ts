/**
 * Mobile drill-down state shared between the root layout (level 1), the agent
 * page (levels 2 and 3) and settings.
 *
 * Level 1 = app navigation, level 2 = a section list, level 3 = content.
 */
class NavState {
/** Level 1 (app sidebar) is the visible screen on mobile. */
level1 = $state(true);

showLevel1() {
this.level1 = true;
}

/** Hide level 1 and show whatever the current route renders. */
hideLevel1() {
this.level1 = false;
}
}

export const navState = new NavState();
