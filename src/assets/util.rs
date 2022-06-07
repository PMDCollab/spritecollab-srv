use itertools::Itertools;

pub fn join_form(form_path: &[i32], with_leading_slash: bool, character: char) -> String {
    let mut form_joined = form_path
        .iter()
        .map(|v| format!("{:04}", v))
        .join(&character.to_string());
    if !form_joined.is_empty() && with_leading_slash {
        form_joined = format!("{}{}", character, form_joined);
    }
    form_joined
}

pub fn join_monster_and_form(monster_idx: i32, form_path: &[i32], character: char) -> String {
    format!(
        "{:04}{}",
        monster_idx,
        join_form(form_path, true, character)
    )
}

/// This is used for shiny recolor routes to render the non-shiny version like SpriteBot does it.
pub fn force_non_shiny_group<'a, I: IntoIterator<Item = &'a i32>>(group: I) -> Vec<i32> {
    let mut collected: Vec<i32> = group.into_iter().copied().collect();
    if collected.len() >= 2 {
        collected[1] = 0;
    }
    while let Some(last) = collected.last() {
        if *last == 0 {
            collected.pop();
        } else {
            break;
        }
    }
    collected
}

/// This is used for shiny recolor routes to render the shiny URLs in the API
/// like SpriteBot does it.
pub fn force_shiny_group<'a, I: IntoIterator<Item = &'a i32>>(group: I) -> Vec<i32> {
    let mut collected: Vec<i32> = group.into_iter().copied().collect();
    if collected.len() >= 2 {
        collected[1] = 1;
    } else if collected.len() == 1 {
        collected.push(1)
    } else {
        collected = vec![0, 1];
    }
    collected
}
