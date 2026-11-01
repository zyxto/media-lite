//! Traducciones de géneros y de idiomas que devuelven las plataformas (en inglés o con códigos).

/// Género en inglés → español. Lo que no está en la tabla queda igual.
pub fn genre_es(genre: &str) -> String {
    let es = match genre {
        "Action" => "Acción",
        "Action & Adventure" => "Acción y aventura",
        "Adventure" => "Aventura",
        "Animation" => "Animación",
        "Anime" => "Anime",
        "Biography" => "Biografía",
        "Children" | "Kids" => "Infantil",
        "Children & Family" => "Infantil y familia",
        "Classics" => "Clásicos",
        "Comedy" => "Comedia",
        "Crime" => "Crimen",
        "Documentaries" => "Documentales",
        "Documentary" => "Documental",
        "Drama" => "Drama",
        "Family" => "Familia",
        "Fantasy" => "Fantasía",
        "Faith & Spirituality" => "Fe",
        "Food" | "Food & Drink" => "Cocina",
        "Game Show" => "Concursos",
        "Gay & Lesbian" => "LGBT",
        "History" => "Historia",
        "Holiday" => "Fiestas",
        "Horror" => "Terror",
        "Independent" => "Independiente",
        "Instructional & Educational" => "Educativo",
        "Martial Arts" => "Artes marciales",
        "Music" => "Música",
        "Musical" => "Musical",
        "Musicals" => "Musicales",
        "Mystery" => "Misterio",
        "News" | "News and Information" => "Noticias",
        "Reality" => "Reality",
        "Romance" => "Romance",
        "Sci-Fi & Fantasy" => "Ciencia ficción y fantasía",
        "Science Fiction" | "Sci-Fi" => "Ciencia ficción",
        "Sport" | "Sports" => "Deportes",
        "Suspense" | "Thriller" | "Thrillers" => "Suspenso",
        "Talk" => "Entrevistas",
        "Travel" => "Viajes",
        "War" => "Bélica",
        "Western" | "Westerns" => "Western",
        other => return other.to_string(),
    };
    es.to_string()
}

/// Nombre de un idioma a partir de su código (`es`, `spa`, `en-US`…). Si no se conoce, el código o `fallback`.
pub fn lang_name(code: &str, fallback: &str) -> String {
    let base: String = code
        .chars()
        .take_while(|c| *c != '-' && *c != '_')
        .map(|c| c.to_ascii_lowercase())
        .collect();
    let name = match base.as_str() {
        "es" | "spa" => "Español",
        "en" | "eng" => "Inglés",
        "pt" | "por" => "Portugués",
        "fr" | "fra" | "fre" => "Francés",
        "it" | "ita" => "Italiano",
        "de" | "deu" | "ger" => "Alemán",
        "ja" | "jpn" => "Japonés",
        "ko" | "kor" => "Coreano",
        "zh" | "chi" | "zho" => "Chino",
        "ru" | "rus" => "Ruso",
        "ar" | "ara" => "Árabe",
        "hi" => "Hindi",
        "nl" => "Neerlandés",
        "tr" => "Turco",
        "pl" => "Polaco",
        "sv" => "Sueco",
        "he" => "Hebreo",
        _ => {
            return if !fallback.is_empty() {
                fallback.to_string()
            } else if !code.is_empty() {
                code.to_string()
            } else {
                "Desconocido".to_string()
            };
        }
    };
    name.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn translates_genres_and_keeps_unknown() {
        assert_eq!(genre_es("Comedy"), "Comedia");
        assert_eq!(genre_es("Cooking Show"), "Cooking Show");
    }

    #[test]
    fn language_names() {
        assert_eq!(lang_name("es-419", ""), "Español");
        assert_eq!(lang_name("xx", ""), "xx");
        assert_eq!(lang_name("", ""), "Desconocido");
        assert_eq!(lang_name("zz", "Bereber"), "Bereber");
    }
}
