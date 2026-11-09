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
	app := NewPickerApp()

	err := wails.Run(&options.App{
		Title:       "",
		Width:       1440,
		Height:      160,
		MinHeight:   160,
		MaxHeight:   160,
		Frameless:   true,
		AlwaysOnTop: true,
		StartHidden: true, // position first, then show

		AssetServer: &assetserver.Options{
			Assets:  assets,
			Handler: app, // serves /local/<path> for wallpaper images
		},

		BackgroundColour: &options.RGBA{R: 36, G: 39, B: 58, A: 220}, // Catppuccin base with transparency

		OnStartup: app.startup,

		Mac: &mac.Options{
			WebviewIsTransparent: true,
			WindowIsTranslucent:  true,
			TitleBar: &mac.TitleBar{
				TitlebarAppearsTransparent: true,
				HideTitle:                 true,
				HideToolbarSeparator:      true,
				FullSizeContent:           true,
			},
		},

		Bind: []interface{}{app},
	})
	if err != nil {
		log.Fatal(err)
	}
}
