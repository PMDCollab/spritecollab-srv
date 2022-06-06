use itertools::Itertools;

pub fn join_form(form_path: &[i32], with_leading_slash: bool) -> String {
    let mut form_joined = form_path.iter().map(|v| format!("{:04}", v)).join("/");
    if !form_joined.is_empty() && with_leading_slash {
        form_joined = format!("/{}", form_joined);
    }
    form_joined
}

pub fn join_monster_and_form(monster_idx: i32, form_path: &[i32]) -> String {
    format!("{:04}{}", monster_idx, join_form(form_path, true))
}
