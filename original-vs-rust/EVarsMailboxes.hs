{-# LANGUAGE ScopedTypeVariables #-}
import Transient.Base
import Transient.Internals
import Transient.Indeterminism
import Control.Monad
import Control.Applicative
import Control.Monad.IO.Class
import Control.Concurrent
import qualified System.Timeout as T

say :: String -> TransIO ()
say = liftIO . putStrLn

run :: String -> TransIO () -> IO ()
run name t = do
  putStrLn ("--- " ++ name)
  r <- T.timeout 2000000 (keep' t)
  case r of
    Nothing -> putStrLn "   (keep' does not finish: listeners remain)"
    Just rs -> putStrLn ("   keep' returns " ++ show (length rs) ++ " results")

main = do
  run "E1 doc: hello world, the last one with lastWriteEVar" $ do
    ev <- newEVar
    r <- readEVar ev <|> (writeEVar ev "hello" >> lastWriteEVar ev "world" >> empty)
    say r

  run "E1b without lastWriteEVar the reader stays alive" $ do
    ev <- newEVar
    r <- readEVar ev <|> (writeEVar ev "hello" >> writeEVar ev "world" >> empty)
    say r

  run "E2 order of the readers (LIFO) and synchronous write" $ do
    ev <- newEVar
    (do r <- readEVar ev; say ("A " ++ r))
      <|> (do r <- readEVar ev; say ("B " ++ r))
      <|> (do writeEVar ev "x"; say "written"; cleanEVar ev; empty)

  run "E3 readEVar1: a single value" $ do
    ev <- newEVar
    (do r <- readEVar1 ev; say ("one " ++ show (r :: Int)))
      <|> (do writeEVar ev 1; writeEVar ev 2; say "written"; empty)

  run "E4 delReadEVar inside the reader" $ do
    ev <- newEVar
    (do r <- readEVar ev; say ("read " ++ show (r :: Int)); delReadEVar ev)
      <|> (do writeEVar ev 1; writeEVar ev 2; say "written"; empty)

  run "E5 a reader that does abduce: the writer does not wait" $ do
    ev <- newEVar
    (do r <- readEVar ev; abduce; liftIO (threadDelay 100000); say ("slow " ++ show (r :: Int)); delReadEVar ev)
      <|> (do writeEVar ev 1; say "written"; empty)

  run "M1 mailbox: put and get by type, deleteMailbox on seeing 3" $ do
    (do x <- getMailbox :: TransIO Int; say ("got " ++ show x); when (x == 3) (deleteMailbox (0 :: Int)))
      <|> (do mapM_ putMailbox [1,2,3 :: Int]; empty)

  run "M2 different mailboxes by type and by key" $ do
    (do x <- getMailbox :: TransIO Int; say ("int " ++ show x); deleteMailbox (0 :: Int))
      <|> (do x <- getMailbox :: TransIO String; say ("string " ++ x); deleteMailbox "")
      <|> (do x <- getMailbox' "a" :: TransIO Int; say ("key a " ++ show x); deleteMailbox' "a" (0 :: Int))
      <|> (do x <- getMailbox' "b" :: TransIO Int; say ("key b " ++ show x); deleteMailbox' "b" (0 :: Int))
      <|> (do liftIO (threadDelay 20000)
              putMailbox (1 :: Int); putMailbox "s"; putMailbox' "a" (10 :: Int); putMailbox' "b" (20 :: Int); empty)

  run "M3 lastPutMailbox terminates the readers" $ do
    (do x <- getMailbox :: TransIO Int; say ("got " ++ show x))
      <|> (do liftIO (threadDelay 20000); putMailbox (1 :: Int); lastPutMailbox (2 :: Int); empty)

  run "M4 a put without readers is lost (there is no queue)" $ do
    (do putMailbox (1 :: Int); say "put 1 without reader"; empty)
      <|> (do liftIO (threadDelay 50000); x <- getMailbox :: TransIO Int; say ("got " ++ show x); deleteMailbox (0 :: Int))
      <|> (do liftIO (threadDelay 150000); putMailbox (2 :: Int); empty)

  run "M5 two readers of the same mailbox: LIFO" $ do
    (do x <- getMailbox :: TransIO Int; say ("A " ++ show x))
      <|> (do x <- getMailbox :: TransIO Int; say ("B " ++ show x))
      <|> (do liftIO (threadDelay 20000); lastPutMailbox (1 :: Int); empty)

  run "M6 getEVarFromMailbox + writeEVar direct" $ do
    (do x <- getMailbox :: TransIO Int; say ("got " ++ show x); deleteMailbox (0 :: Int))
      <|> (do liftIO (threadDelay 20000); ev <- getEVarFromMailbox :: TransIO (EVar Int); writeEVar ev 5; empty)
