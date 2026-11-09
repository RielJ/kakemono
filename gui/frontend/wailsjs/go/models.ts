export namespace main {
	
	export class APODConfigDTO {
	    enabled: boolean;
	    api_key: string;
	
	    static createFrom(source: any = {}) {
	        return new APODConfigDTO(source);
	    }
	
	    constructor(source: any = {}) {
	        if ('string' === typeof source) source = JSON.parse(source);
	        this.enabled = source["enabled"];
	        this.api_key = source["api_key"];
	    }
	}
	export class BingConfigDTO {
	    enabled: boolean;
	    market: string;
	
	    static createFrom(source: any = {}) {
	        return new BingConfigDTO(source);
	    }
	
	    constructor(source: any = {}) {
	        if ('string' === typeof source) source = JSON.parse(source);
	        this.enabled = source["enabled"];
	        this.market = source["market"];
	    }
	}
	export class PickerConfigDTO {
	    position: string;
	
	    static createFrom(source: any = {}) {
	        return new PickerConfigDTO(source);
	    }
	
	    constructor(source: any = {}) {
	        if ('string' === typeof source) source = JSON.parse(source);
	        this.position = source["position"];
	    }
	}
	export class FavoritesConfigDTO {
	    dir: string;
	
	    static createFrom(source: any = {}) {
	        return new FavoritesConfigDTO(source);
	    }
	
	    constructor(source: any = {}) {
	        if ('string' === typeof source) source = JSON.parse(source);
	        this.dir = source["dir"];
	    }
	}
	export class ThemeConfigDTO {
	    sources: string[];
	    queries: string[];
	
	    static createFrom(source: any = {}) {
	        return new ThemeConfigDTO(source);
	    }
	
	    constructor(source: any = {}) {
	        if ('string' === typeof source) source = JSON.parse(source);
	        this.sources = source["sources"];
	        this.queries = source["queries"];
	    }
	}
	export class LocalConfigDTO {
	    enabled: boolean;
	    dirs: string[];
	    recursive: boolean;
	
	    static createFrom(source: any = {}) {
	        return new LocalConfigDTO(source);
	    }
	
	    constructor(source: any = {}) {
	        if ('string' === typeof source) source = JSON.parse(source);
	        this.enabled = source["enabled"];
	        this.dirs = source["dirs"];
	        this.recursive = source["recursive"];
	    }
	}
	export class RedditConfigDTO {
	    enabled: boolean;
	    subreddits: string[];
	
	    static createFrom(source: any = {}) {
	        return new RedditConfigDTO(source);
	    }
	
	    constructor(source: any = {}) {
	        if ('string' === typeof source) source = JSON.parse(source);
	        this.enabled = source["enabled"];
	        this.subreddits = source["subreddits"];
	    }
	}
	export class UnsplashConfigDTO {
	    enabled: boolean;
	    access_key: string;
	
	    static createFrom(source: any = {}) {
	        return new UnsplashConfigDTO(source);
	    }
	
	    constructor(source: any = {}) {
	        if ('string' === typeof source) source = JSON.parse(source);
	        this.enabled = source["enabled"];
	        this.access_key = source["access_key"];
	    }
	}
	export class WallhavenConfigDTO {
	    enabled: boolean;
	    api_key: string;
	    purity: string;
	    sorting: string;
	    categories: string;
	
	    static createFrom(source: any = {}) {
	        return new WallhavenConfigDTO(source);
	    }
	
	    constructor(source: any = {}) {
	        if ('string' === typeof source) source = JSON.parse(source);
	        this.enabled = source["enabled"];
	        this.api_key = source["api_key"];
	        this.purity = source["purity"];
	        this.sorting = source["sorting"];
	        this.categories = source["categories"];
	    }
	}
	export class DaemonConfigDTO {
	    rotation_enabled: boolean;
	    interval: string;
	    download_dir: string;
	    cache_size: number;
	    trash_dir: string;
	    socket_path: string;
	
	    static createFrom(source: any = {}) {
	        return new DaemonConfigDTO(source);
	    }
	
	    constructor(source: any = {}) {
	        if ('string' === typeof source) source = JSON.parse(source);
	        this.rotation_enabled = source["rotation_enabled"];
	        this.interval = source["interval"];
	        this.download_dir = source["download_dir"];
	        this.cache_size = source["cache_size"];
	        this.trash_dir = source["trash_dir"];
	        this.socket_path = source["socket_path"];
	    }
	}
	export class ConfigDTO {
	    daemon: DaemonConfigDTO;
	    wallhaven: WallhavenConfigDTO;
	    unsplash: UnsplashConfigDTO;
	    reddit: RedditConfigDTO;
	    bing: BingConfigDTO;
	    apod: APODConfigDTO;
	    local: LocalConfigDTO;
	    light: ThemeConfigDTO;
	    dark: ThemeConfigDTO;
	    favorites: FavoritesConfigDTO;
	    picker: PickerConfigDTO;
	
	    static createFrom(source: any = {}) {
	        return new ConfigDTO(source);
	    }
	
	    constructor(source: any = {}) {
	        if ('string' === typeof source) source = JSON.parse(source);
	        this.daemon = this.convertValues(source["daemon"], DaemonConfigDTO);
	        this.wallhaven = this.convertValues(source["wallhaven"], WallhavenConfigDTO);
	        this.unsplash = this.convertValues(source["unsplash"], UnsplashConfigDTO);
	        this.reddit = this.convertValues(source["reddit"], RedditConfigDTO);
	        this.bing = this.convertValues(source["bing"], BingConfigDTO);
	        this.apod = this.convertValues(source["apod"], APODConfigDTO);
	        this.local = this.convertValues(source["local"], LocalConfigDTO);
	        this.light = this.convertValues(source["light"], ThemeConfigDTO);
	        this.dark = this.convertValues(source["dark"], ThemeConfigDTO);
	        this.favorites = this.convertValues(source["favorites"], FavoritesConfigDTO);
	        this.picker = this.convertValues(source["picker"], PickerConfigDTO);
	    }
	
		convertValues(a: any, classs: any, asMap: boolean = false): any {
		    if (!a) {
		        return a;
		    }
		    if (a.slice && a.map) {
		        return (a as any[]).map(elem => this.convertValues(elem, classs));
		    } else if ("object" === typeof a) {
		        if (asMap) {
		            for (const key of Object.keys(a)) {
		                a[key] = new classs(a[key]);
		            }
		            return a;
		        }
		        return new classs(a);
		    }
		    return a;
		}
	}
	
	
	
	
	
	export class WallpaperDTO {
	    path: string;
	    source: string;
	    source_url: string;
	    image_url: string;
	    title: string;
	    author: string;
	    width: number;
	    height: number;
	    set_at: string;
	
	    static createFrom(source: any = {}) {
	        return new WallpaperDTO(source);
	    }
	
	    constructor(source: any = {}) {
	        if ('string' === typeof source) source = JSON.parse(source);
	        this.path = source["path"];
	        this.source = source["source"];
	        this.source_url = source["source_url"];
	        this.image_url = source["image_url"];
	        this.title = source["title"];
	        this.author = source["author"];
	        this.width = source["width"];
	        this.height = source["height"];
	        this.set_at = source["set_at"];
	    }
	}
	export class StatusDTO {
	    running: boolean;
	    current?: WallpaperDTO;
	    paused: boolean;
	    interval: string;
	    theme: string;
	    history_count: number;
	    history_index: number;
	
	    static createFrom(source: any = {}) {
	        return new StatusDTO(source);
	    }
	
	    constructor(source: any = {}) {
	        if ('string' === typeof source) source = JSON.parse(source);
	        this.running = source["running"];
	        this.current = this.convertValues(source["current"], WallpaperDTO);
	        this.paused = source["paused"];
	        this.interval = source["interval"];
	        this.theme = source["theme"];
	        this.history_count = source["history_count"];
	        this.history_index = source["history_index"];
	    }
	
		convertValues(a: any, classs: any, asMap: boolean = false): any {
		    if (!a) {
		        return a;
		    }
		    if (a.slice && a.map) {
		        return (a as any[]).map(elem => this.convertValues(elem, classs));
		    } else if ("object" === typeof a) {
		        if (asMap) {
		            for (const key of Object.keys(a)) {
		                a[key] = new classs(a[key]);
		            }
		            return a;
		        }
		        return new classs(a);
		    }
		    return a;
		}
	}
	
	
	

}

