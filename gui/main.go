package main

import (
	"embed"
	"log"

	"github.com/wailsapp/wails/v2"
	"github.com/wailsapp/wails/v2/pkg/options"
	"github.com/wailsapp/wails/v2/pkg/options/assetserver"
	"github.com/wailsapp/wails/v2/pkg/options/mac"
)

//go:embed all:frontend/dist
var assets embed.FS

func main() {
	app := NewSettingsApp()

	err := wails.Run(&options.App{
		Title:     "wallpaperd Settings",
		Width:     760,
		Height:    600,
		MinWidth:  640,
		MinHeight: 520,
		AssetServer: &assetserver.Options{
			Assets:  assets,
			Handler: FileHandler{},
		},
		OnStartup: app.startup,
		Bind: []interface{}{
			app,
		},
		Mac: &mac.Options{
			TitleBar:             mac.TitleBarHiddenInset(),
			WebviewIsTransparent: true,
			WindowIsTranslucent:  true,
			About: &mac.AboutInfo{
				Title:   "wallpaperd Settings",
				Message: "Wallpaper manager settings",
			},
		},
		// Disable the default frameless window so native title bar is used
		Frameless:        false,
		CSSDragProperty:  "--wails-draggable",
		CSSDragValue:     "drag",
	})

	if err != nil {
		log.Fatal(err)
	}
}
